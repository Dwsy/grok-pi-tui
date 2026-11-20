mod protocol;
mod runner;
mod trace;

use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use protocol::{HandshakeInfo, HostEvent, MAX_REQUEST_BYTES, ProtocolError, RunRequest};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "--handshake") {
        let info = HandshakeInfo::current();
        println!(
            "{}",
            serde_json::to_string(&info).expect("handshake serializes")
        );
        return ExitCode::SUCCESS;
    }

    let cancel_flag = Arc::new(AtomicBool::new(false));
    install_signal_handlers(Arc::clone(&cancel_flag));

    let req = match read_single_request() {
        Ok(req) => req,
        Err(err) => {
            emit_event(&HostEvent::Error {
                code: err.code,
                message: err.message,
            });
            return ExitCode::from(1);
        }
    };

    if let Err(err) = req.validate() {
        emit_event(&HostEvent::Error {
            code: err.code,
            message: err.message,
        });
        return ExitCode::from(1);
    }

    match runner::execute_request(req, cancel_flag, |ev| emit_event(&ev)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            emit_event(&HostEvent::Error {
                code: err.code,
                message: err.message,
            });
            ExitCode::from(1)
        }
    }
}

fn read_single_request() -> Result<RunRequest, ProtocolError> {
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    let mut line = String::new();
    let bytes = handle
        .read_line(&mut line)
        .map_err(|e| ProtocolError::new("stdin_read_error", e.to_string()))?;

    if bytes == 0 || line.trim().is_empty() {
        return Err(ProtocolError::new(
            "empty_request",
            "Expected one JSONL RunRequest line on stdin",
        ));
    }
    if line.len() > MAX_REQUEST_BYTES {
        return Err(ProtocolError::new(
            "request_too_large",
            format!("Request exceeds {} byte limit", MAX_REQUEST_BYTES),
        ));
    }

    serde_json::from_str::<RunRequest>(line.trim())
        .map_err(|e| ProtocolError::new("invalid_json", e.to_string()))
}

fn emit_event(event: &HostEvent) {
    if let Ok(json) = serde_json::to_string(event) {
        let mut stdout = io::stdout().lock();
        let _ = writeln!(stdout, "{json}");
        let _ = stdout.flush();
    }
}

#[cfg(unix)]
fn install_signal_handlers(cancel_flag: Arc<AtomicBool>) {
    tokio::spawn(async move {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        let mut sigint =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();

        tokio::select! {
            _ = async {
                if let Some(ref mut s) = sigterm {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
            _ = async {
                if let Some(ref mut s) = sigint {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
        }
        cancel_flag.store(true, Ordering::Release);
    });
}

#[cfg(not(unix))]
fn install_signal_handlers(_cancel_flag: Arc<AtomicBool>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn handshake_info_has_expected_versions() {
        let info = HandshakeInfo::current();
        assert_eq!(info.protocol_version, protocol::PROTOCOL_VERSION);
        assert_eq!(info.trace_version, protocol::TRACE_SCHEMA_VERSION);
        assert_eq!(info.engine, "omp-shell");
    }

    #[test]
    fn run_request_validates_protocol_and_cwd() {
        let tmp = TempDir::new().unwrap();
        let valid = RunRequest {
            protocol_version: protocol::PROTOCOL_VERSION,
            command: "echo ok".to_string(),
            cwd: tmp.path().to_string_lossy().to_string(),
            env: Default::default(),
            timeout_ms: Some(5000),
            trace: true,
        };
        assert!(valid.validate().is_ok());

        let bad_ver = RunRequest {
            protocol_version: 99,
            ..valid.clone()
        };
        assert_eq!(
            bad_ver.validate().unwrap_err().code,
            "unsupported_protocol_version"
        );

        let bad_cwd = RunRequest {
            cwd: "relative/path".to_string(),
            ..valid
        };
        assert_eq!(bad_cwd.validate().unwrap_err().code, "invalid_cwd");
    }
}
