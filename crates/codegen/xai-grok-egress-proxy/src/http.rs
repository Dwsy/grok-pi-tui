use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use xai_grok_sandbox::WebsiteOrigin;
use xai_grok_sandbox::command::CommandTag;

use crate::error::{ConnectionError, ProxyError, write_committed};
use crate::metrics::{ProxyMetrics, ProxyOutcome};
use crate::request::{ParsedRequest, parse_http_target, read_head, static_hop_by_hop};
use crate::{BoxProxyIo, EgressProxyOptions, ProxyIo, ProxyState};

pub(crate) async fn handle<S: ProxyIo>(
    client: &mut S,
    request: ParsedRequest,
    mut buffered: Vec<u8>,
    state: &ProxyState,
    call: Option<&CommandTag>,
) -> Result<(), ConnectionError> {
    let options = &state.options;
    let metrics = &state.metrics;
    let (authority, target) =
        parse_http_target(&request.target).map_err(ConnectionError::Precommit)?;
    let origin = WebsiteOrigin::parse(&format!("http://{authority}"))
        .map_err(|_| ConnectionError::Precommit(ProxyError::Malformed))?;
    let host = request
        .single_header("host")
        .map_err(ConnectionError::Precommit)?
        .ok_or(ConnectionError::Precommit(ProxyError::Malformed))?;
    if crate::request::normalize_authority(host, "http").map_err(ConnectionError::Precommit)?
        != authority
    {
        return Err(ConnectionError::Precommit(ProxyError::Malformed));
    }
    let content_length = request.framing().map_err(ConnectionError::Precommit)?;
    let length = content_length.unwrap_or(0);
    if length > 0 {
        return Err(ConnectionError::Precommit(ProxyError::BodyUnsupported));
    }
    if !buffered.is_empty() {
        return Err(ConnectionError::Precommit(ProxyError::Malformed));
    }
    state
        .admit(&origin, call, client, &mut buffered)
        .await
        .map_err(ConnectionError::from_admit)?;
    // A bodiless request has nothing more to say; bytes sent during the hold are pipelining.
    if !buffered.is_empty() {
        return Err(ConnectionError::Precommit(ProxyError::Malformed));
    }

    let mut upstream = crate::resolve_and_connect(&origin, options)
        .await
        .map_err(ConnectionError::Precommit)?;
    let headers = request
        .sanitized_headers(&authority, content_length)
        .map_err(ConnectionError::Precommit)?;
    let mut outbound = format!("{} {} HTTP/1.1\r\n", request.method, target).into_bytes();
    for (name, value) in headers {
        outbound.extend_from_slice(name.as_bytes());
        outbound.extend_from_slice(b": ");
        outbound.extend_from_slice(value.as_bytes());
        outbound.extend_from_slice(b"\r\n");
        if outbound.len() > options.max_header_bytes {
            return Err(ConnectionError::Precommit(ProxyError::TooLarge));
        }
    }
    outbound.extend_from_slice(b"\r\n");
    upstream
        .write_all(&outbound)
        .await
        .map_err(|_| ConnectionError::Precommit(ProxyError::Connect))?;
    forward_response(upstream, client, &request.method, options, metrics).await
}

async fn forward_response<S: ProxyIo>(
    mut upstream: BoxProxyIo,
    client: &mut S,
    request_method: &str,
    options: &EgressProxyOptions,
    metrics: &ProxyMetrics,
) -> Result<(), ConnectionError> {
    let (head, buffered) = tokio::time::timeout(
        options.request_timeout,
        read_head(&mut upstream, options.max_header_bytes),
    )
    .await
    .map_err(|_| ConnectionError::Precommit(ProxyError::Timeout))?
    .map_err(|_| ConnectionError::Precommit(ProxyError::Connect))?;
    let response =
        ParsedResponse::parse(&head, options.max_headers).map_err(ConnectionError::Precommit)?;
    let body = response
        .body_mode(request_method, !buffered.is_empty())
        .map_err(ConnectionError::Precommit)?;
    let headers = response
        .sanitized_headers(body)
        .map_err(ConnectionError::Precommit)?;
    let mut outbound = format!("HTTP/1.1 {} {}\r\n", response.status, response.reason).into_bytes();
    for (name, value) in headers {
        outbound.extend_from_slice(name.as_bytes());
        outbound.extend_from_slice(b": ");
        outbound.extend_from_slice(value.as_bytes());
        outbound.extend_from_slice(b"\r\n");
    }
    outbound.extend_from_slice(b"\r\n");
    write_committed(client, &outbound).await?;

    match body {
        ResponseBody::None(_) | ResponseBody::Reset { buffered: false } => {
            metrics.record(ProxyOutcome::Ok);
            Ok(())
        }
        ResponseBody::Reset { buffered: true } => {
            Err(ConnectionError::Committed(ProxyError::Connect))
        }
        ResponseBody::Length(length) => {
            let buffered_len = buffered.len() as u64;
            if buffered_len > length {
                return Err(ConnectionError::Committed(ProxyError::Malformed));
            }
            write_committed(client, &buffered).await?;
            copy_exact(
                &mut upstream,
                client,
                length - buffered_len,
                options.request_timeout,
            )
            .await?;
            metrics.record(ProxyOutcome::Ok);
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
enum ResponseBody {
    None(Option<u64>),
    Reset { buffered: bool },
    Length(u64),
}

struct ParsedResponse {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
}

impl ParsedResponse {
    fn parse(bytes: &[u8], max_headers: usize) -> Result<Self, ProxyError> {
        let mut raw_headers = vec![httparse::EMPTY_HEADER; max_headers];
        let mut response = httparse::Response::new(&mut raw_headers);
        let parsed = response.parse(bytes).map_err(|error| match error {
            httparse::Error::TooManyHeaders => ProxyError::TooLarge,
            _ => ProxyError::Connect,
        })?;
        if !parsed.is_complete() || response.version != Some(1) {
            return Err(ProxyError::Connect);
        }
        let status = response.code.ok_or(ProxyError::Connect)?;
        if status < 200 {
            return Err(ProxyError::Connect);
        }
        let reason = response
            .reason
            .filter(|reason| {
                reason
                    .bytes()
                    .all(|byte| byte == b'\t' || (b' '..=b'~').contains(&byte))
            })
            .unwrap_or("Response")
            .to_owned();
        let mut headers = Vec::with_capacity(response.headers.len());
        for header in response.headers {
            let value = std::str::from_utf8(header.value).map_err(|_| ProxyError::Connect)?;
            if value.bytes().any(|byte| byte < b' ' && byte != b'\t') {
                return Err(ProxyError::Connect);
            }
            headers.push((header.name.to_owned(), value.trim().to_owned()));
        }
        Ok(Self {
            status,
            reason,
            headers,
        })
    }

    fn single_header(&self, name: &str) -> Result<Option<&str>, ProxyError> {
        let mut values = self
            .headers
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name));
        let value = values.next().map(|(_, value)| value.as_str());
        if values.next().is_some() {
            return Err(ProxyError::Connect);
        }
        Ok(value)
    }

    fn representation_length(&self) -> Result<Option<u64>, ProxyError> {
        if self.single_header("transfer-encoding")?.is_some() {
            return Err(ProxyError::Connect);
        }
        self.single_header("content-length")?
            .map(|value| {
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(ProxyError::Connect);
                }
                value.parse().map_err(|_| ProxyError::Connect)
            })
            .transpose()
    }

    fn body_mode(
        &self,
        request_method: &str,
        buffered_body: bool,
    ) -> Result<ResponseBody, ProxyError> {
        let length = self.representation_length()?;
        if request_method.eq_ignore_ascii_case("HEAD") || self.status == 304 {
            if buffered_body {
                return Err(ProxyError::Connect);
            }
            return Ok(ResponseBody::None(length));
        }
        if self.status == 204 {
            if buffered_body || length.is_some() {
                return Err(ProxyError::Connect);
            }
            return Ok(ResponseBody::None(None));
        }
        if self.status == 205 {
            if length.is_some_and(|length| length != 0) {
                return Err(ProxyError::Connect);
            }
            return Ok(ResponseBody::Reset {
                buffered: buffered_body,
            });
        }
        Ok(ResponseBody::Length(length.ok_or(ProxyError::Connect)?))
    }

    fn sanitized_headers(&self, body: ResponseBody) -> Result<Vec<(String, String)>, ProxyError> {
        let mut connection_tokens = std::collections::BTreeSet::new();
        for (_, value) in self
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        {
            for token in value.split(',') {
                let token = token.trim().to_ascii_lowercase();
                if token.is_empty() {
                    return Err(ProxyError::Connect);
                }
                connection_tokens.insert(token);
            }
        }
        let mut output = Vec::new();
        for (name, value) in &self.headers {
            let lower = name.to_ascii_lowercase();
            if lower == "content-length"
                || static_hop_by_hop(&lower)
                || connection_tokens.contains(&lower)
            {
                continue;
            }
            output.push((name.clone(), value.clone()));
        }
        match body {
            ResponseBody::None(Some(length)) | ResponseBody::Length(length) => {
                output.push(("Content-Length".to_owned(), length.to_string()));
            }
            ResponseBody::None(None) | ResponseBody::Reset { .. } => {}
        }
        output.push(("Connection".to_owned(), "close".to_owned()));
        Ok(output)
    }
}

async fn copy_exact<S: ProxyIo>(
    source: &mut BoxProxyIo,
    destination: &mut S,
    mut remaining: u64,
    timeout: Duration,
) -> Result<(), ConnectionError> {
    tokio::time::timeout(timeout, async move {
        let mut buffer = [0u8; 16 * 1024];
        while remaining > 0 {
            let limit = remaining.min(buffer.len() as u64) as usize;
            let count = source
                .read(&mut buffer[..limit])
                .await
                .map_err(|_| ConnectionError::Committed(ProxyError::Connect))?;
            if count == 0 {
                return Err(ConnectionError::Committed(ProxyError::Connect));
            }
            destination
                .write_all(&buffer[..count])
                .await
                .map_err(ProxyError::from)
                .map_err(ConnectionError::Committed)?;
            remaining -= count as u64;
        }
        Ok(())
    })
    .await
    .map_err(|_| ConnectionError::Committed(ProxyError::Timeout))?
}
