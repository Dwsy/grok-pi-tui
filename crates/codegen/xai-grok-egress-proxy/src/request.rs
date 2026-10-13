use std::collections::BTreeSet;

use tokio::io::{AsyncRead, AsyncReadExt};
use xai_grok_sandbox::WebsiteOrigin;
use xai_grok_sandbox::command::grants::split_host_port;

use crate::error::ProxyError;

pub(crate) struct ParsedRequest {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}

impl ParsedRequest {
    pub(crate) fn parse(bytes: &[u8], max_headers: usize) -> Result<Self, ProxyError> {
        let mut raw_headers = vec![httparse::EMPTY_HEADER; max_headers];
        let mut request = httparse::Request::new(&mut raw_headers);
        let parsed = request.parse(bytes).map_err(|error| match error {
            httparse::Error::TooManyHeaders => ProxyError::TooLarge,
            _ => ProxyError::Malformed,
        })?;
        if !parsed.is_complete() || request.version != Some(1) {
            return Err(ProxyError::Malformed);
        }
        let method = request.method.ok_or(ProxyError::Malformed)?;
        if !method
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte == b'-')
        {
            return Err(ProxyError::Malformed);
        }
        let target = request.path.ok_or(ProxyError::Malformed)?;
        let mut headers = Vec::with_capacity(request.headers.len());
        for header in request.headers {
            let value = std::str::from_utf8(header.value).map_err(|_| ProxyError::Malformed)?;
            if value.bytes().any(|byte| byte < b' ' && byte != b'\t') {
                return Err(ProxyError::Malformed);
            }
            headers.push((header.name.to_owned(), value.trim().to_owned()));
        }
        Ok(Self {
            method: method.to_owned(),
            target: target.to_owned(),
            headers,
        })
    }

    pub(crate) fn single_header(&self, name: &str) -> Result<Option<&str>, ProxyError> {
        let mut values = self
            .headers
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name));
        let value = values.next().map(|(_, value)| value.as_str());
        if values.next().is_some() {
            return Err(ProxyError::Malformed);
        }
        Ok(value)
    }

    pub(crate) fn framing(&self) -> Result<Option<u64>, ProxyError> {
        if self.single_header("transfer-encoding")?.is_some()
            || self.single_header("expect")?.is_some()
        {
            return Err(ProxyError::Malformed);
        }
        let length = self
            .single_header("content-length")?
            .map(parse_content_length)
            .transpose()?;
        self.connection_tokens()?;
        Ok(length)
    }

    pub(crate) fn connection_tokens(&self) -> Result<BTreeSet<String>, ProxyError> {
        let mut tokens = BTreeSet::new();
        for (_, value) in self
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        {
            for token in value.split(',') {
                let token = token.trim().to_ascii_lowercase();
                if token.is_empty()
                    || !token
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    || critical_connection_token(&token)
                {
                    return Err(ProxyError::Malformed);
                }
                tokens.insert(token);
            }
        }
        Ok(tokens)
    }

    pub(crate) fn sanitized_headers(
        &self,
        authority: &str,
        content_length: Option<u64>,
    ) -> Result<Vec<(String, String)>, ProxyError> {
        let connection_tokens = self.connection_tokens()?;
        let mut output = vec![("Host".to_owned(), authority.to_owned())];
        for (name, value) in &self.headers {
            let lower = name.to_ascii_lowercase();
            if static_hop_by_hop(&lower)
                || lower == "host"
                || lower == "content-length"
                || lower == "proxy-authorization"
                || lower == "forwarded"
                || lower == "via"
                || lower.starts_with("x-forwarded-")
                || connection_tokens.contains(&lower)
            {
                continue;
            }
            output.push((name.clone(), value.clone()));
        }
        if let Some(length) = content_length {
            output.push(("Content-Length".to_owned(), length.to_string()));
        }
        output.push(("Connection".to_owned(), "close".to_owned()));
        Ok(output)
    }
}

fn parse_content_length(value: &str) -> Result<u64, ProxyError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ProxyError::Malformed);
    }
    value.parse().map_err(|_| ProxyError::Malformed)
}

fn critical_connection_token(token: &str) -> bool {
    matches!(
        token,
        "host"
            | "content-length"
            | "transfer-encoding"
            | "expect"
            | "proxy-authorization"
            | "proxy-connection"
            | "connection"
            | "te"
            | "trailer"
            | "upgrade"
    )
}

pub(crate) fn static_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authentication-info"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

pub(crate) async fn read_head<S: AsyncRead + Unpin>(
    stream: &mut S,
    max: usize,
) -> Result<(Vec<u8>, Vec<u8>), ProxyError> {
    let mut bytes = Vec::new();
    loop {
        if let Some(end) = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
        {
            if end > max {
                return Err(ProxyError::TooLarge);
            }
            return Ok((bytes[..end].to_vec(), bytes[end..].to_vec()));
        }
        if bytes.len() >= max {
            return Err(ProxyError::TooLarge);
        }
        let mut buffer = [0u8; 4096];
        let limit = (max - bytes.len()).min(buffer.len());
        let count = stream.read(&mut buffer[..limit]).await?;
        if count == 0 {
            return Err(ProxyError::Malformed);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

pub(crate) fn parse_http_target(value: &str) -> Result<(String, String), ProxyError> {
    let rest = value.strip_prefix("http://").ok_or(ProxyError::Malformed)?;
    if rest.contains('#') || rest.contains('@') || rest.contains('\\') || rest.starts_with(':') {
        return Err(ProxyError::Malformed);
    }
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let raw_authority = &rest[..split];
    if raw_authority.is_empty() {
        return Err(ProxyError::Malformed);
    }
    let authority = normalize_authority(raw_authority, "http")?;
    let suffix = &rest[split..];
    let target = if suffix.is_empty() {
        "/".to_owned()
    } else if suffix.starts_with('?') {
        format!("/{suffix}")
    } else {
        suffix.to_owned()
    };
    Ok((authority, target))
}

pub(crate) fn normalize_authority(value: &str, scheme: &str) -> Result<String, ProxyError> {
    let origin =
        WebsiteOrigin::parse(&format!("{scheme}://{value}")).map_err(|_| ProxyError::Malformed)?;
    Ok(format!("{}:{}", origin.hostname(), origin.port()))
}

pub(crate) fn strict_connect_authority(value: &str) -> Result<String, ProxyError> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('@')
        || value.contains('[')
        || value.contains(']')
        || value.starts_with(':')
        || value.contains('?')
        || value.contains('#')
        || value.contains('\\')
        || value.bytes().any(|byte| byte <= b' ' || byte == 0x7f)
    {
        return Err(ProxyError::Malformed);
    }
    let (host, port) = split_host_port(value);
    let port: u16 = port
        .ok_or(ProxyError::Malformed)?
        .parse()
        .map_err(|_| ProxyError::Malformed)?;
    if host.is_empty() || port == 0 {
        return Err(ProxyError::Malformed);
    }
    normalize_authority(value, "https")
}
