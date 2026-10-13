use tokio::io::{AsyncWriteExt, copy_bidirectional};
use xai_grok_sandbox::WebsiteOrigin;
use xai_grok_sandbox::command::CommandTag;

use crate::error::{ConnectionError, ProxyError, write_committed};
use crate::metrics::ProxyOutcome;
use crate::request::{ParsedRequest, strict_connect_authority};
use crate::tls::read_client_hello;
use crate::{ProxyIo, ProxyState};

pub(crate) async fn handle<S: ProxyIo>(
    client: &mut S,
    request: ParsedRequest,
    mut buffered: Vec<u8>,
    state: &ProxyState,
    call: Option<&CommandTag>,
) -> Result<(), ConnectionError> {
    let options = &state.options;
    let framing = request.framing().map_err(ConnectionError::Precommit)?;
    if framing.is_some() {
        return Err(ConnectionError::Precommit(ProxyError::Malformed));
    }
    let authority =
        strict_connect_authority(&request.target).map_err(ConnectionError::Precommit)?;
    if let Some(host) = request
        .single_header("host")
        .map_err(ConnectionError::Precommit)?
        && strict_connect_authority(host).map_err(ConnectionError::Precommit)? != authority
    {
        return Err(ConnectionError::Precommit(ProxyError::Malformed));
    }
    let origin = WebsiteOrigin::parse(&format!("https://{authority}"))
        .map_err(|_| ConnectionError::Precommit(ProxyError::Malformed))?;
    state
        .admit(&origin, call, client, &mut buffered)
        .await
        .map_err(ConnectionError::from_admit)?;
    let addresses = crate::resolve_public(&origin, options)
        .await
        .map_err(ConnectionError::Precommit)?;
    let mut upstream = tokio::time::timeout(
        options.connect_timeout,
        options.connector.connect(&addresses),
    )
    .await
    .map_err(|_| ConnectionError::Precommit(ProxyError::Timeout))?
    .map_err(|_| ConnectionError::Precommit(ProxyError::Connect))?;

    write_committed(client, b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;

    let hello_timeout = options.tls_hello_timeout.min(options.request_timeout);
    let prefix = read_client_hello(
        client,
        buffered,
        options.max_tls_client_hello_bytes,
        hello_timeout,
        &origin,
    )
    .await
    .map_err(ConnectionError::Committed)?;
    upstream
        .write_all(&prefix)
        .await
        .map_err(|_| ConnectionError::Committed(ProxyError::Connect))?;
    match copy_bidirectional(client, &mut upstream).await {
        Ok(_) => {
            state.metrics.record(ProxyOutcome::Ok);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            state.metrics.record(ProxyOutcome::Ok);
            Ok(())
        }
        Err(_) => Err(ConnectionError::Committed(ProxyError::Connect)),
    }
}
