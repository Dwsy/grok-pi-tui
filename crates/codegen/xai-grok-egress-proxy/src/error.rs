use std::io;

use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::metrics::ProxyOutcome;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProxyError {
    #[error("invalid proxy options")]
    InvalidOptions,
    #[error("malformed proxy request")]
    Malformed,
    #[error("proxy request exceeds configured bounds")]
    TooLarge,
    #[error("request bodies are unsupported by the strict relay subset")]
    BodyUnsupported,
    #[error("proxy authentication required")]
    Authentication,
    #[error("website policy denied the origin")]
    PolicyDenied,
    #[error("the decider did not answer before the hold deadline")]
    HoldTimeout,
    #[error("the client hung up while its request was held")]
    ClientGone,
    #[error("resolved address set is forbidden")]
    AddressDenied,
    #[error("DNS resolution failed")]
    Dns,
    #[error("upstream connection failed")]
    Connect,
    #[error("proxy operation timed out")]
    Timeout,
    #[error("proxy connection limit reached")]
    Overloaded,
    #[error("TLS ClientHello is invalid or missing")]
    TlsClientHello,
    #[error("TLS SNI is missing or mismatched")]
    TlsSni,
    #[error("encrypted ClientHello is unsupported")]
    TlsEch,
    #[error("proxy shutdown timed out")]
    ShutdownTimeout,
    #[error("downstream I/O failure: {0}")]
    DownstreamIo(#[from] io::Error),
}

impl ProxyError {
    pub(crate) fn outcome(&self) -> ProxyOutcome {
        match self {
            Self::Authentication => ProxyOutcome::Unauthenticated,
            Self::PolicyDenied => ProxyOutcome::PolicyDenied,
            Self::HoldTimeout => ProxyOutcome::HoldTimeout,
            Self::ClientGone => ProxyOutcome::Abandoned,
            Self::AddressDenied => ProxyOutcome::AddressDenied,
            Self::Dns => ProxyOutcome::DnsFailed,
            Self::Connect => ProxyOutcome::ConnectFailed,
            Self::Timeout => ProxyOutcome::Timeout,
            Self::Overloaded => ProxyOutcome::Overloaded,
            Self::TlsClientHello | Self::TlsSni | Self::TlsEch => ProxyOutcome::TlsDenied,
            Self::InvalidOptions
            | Self::Malformed
            | Self::TooLarge
            | Self::BodyUnsupported
            | Self::DownstreamIo(_) => ProxyOutcome::Malformed,
            Self::ShutdownTimeout => ProxyOutcome::Timeout,
        }
    }

    pub(crate) fn status(&self) -> (u16, &'static str) {
        match self {
            Self::Authentication => (407, "proxy_authentication_required"),
            Self::PolicyDenied
            | Self::HoldTimeout
            | Self::AddressDenied
            | Self::TlsSni
            | Self::TlsEch => (403, "denied"),
            Self::TlsClientHello => (400, "malformed"),
            Self::Dns | Self::Connect => (502, "upstream_failure"),
            Self::ClientGone | Self::DownstreamIo(_) => (400, "malformed"),
            Self::Overloaded => (503, "overloaded"),
            Self::Timeout | Self::ShutdownTimeout => (504, "timeout"),
            Self::InvalidOptions | Self::Malformed | Self::TooLarge | Self::BodyUnsupported => {
                (400, "malformed")
            }
        }
    }
}

/// `Precommit` errors still get an HTTP error response; `Committed` ones happened after bytes
/// reached the client, so only the metric is recorded.
pub(crate) enum ConnectionError {
    Precommit(ProxyError),
    Committed(ProxyError),
}

impl ConnectionError {
    /// A client that hung up while held gets no response; every other admission failure does.
    pub(crate) fn from_admit(error: ProxyError) -> ConnectionError {
        match error {
            ProxyError::ClientGone => ConnectionError::Committed(error),
            _ => ConnectionError::Precommit(error),
        }
    }

    pub(crate) fn outcome(&self) -> ProxyOutcome {
        match self {
            Self::Precommit(error) | Self::Committed(error) => error.outcome(),
        }
    }
}

pub(crate) async fn write_committed<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
) -> Result<(), ConnectionError> {
    writer
        .write_all(bytes)
        .await
        .map_err(ProxyError::from)
        .map_err(ConnectionError::Committed)
}

pub(crate) async fn write_error<W: AsyncWrite + Unpin>(
    stream: &mut W,
    error: &ProxyError,
) -> Result<(), io::Error> {
    let (status, reason) = error.status();
    let auth = if status == 407 {
        "Proxy-Authenticate: Basic realm=\"grok-egress\", Bearer\r\n"
    } else {
        ""
    };
    let body = format!("{reason}\n");
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n{auth}Content-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await
}
