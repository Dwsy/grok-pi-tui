use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use xai_grok_sandbox::WebsiteOrigin;

use crate::ProxyError;

pub(crate) async fn read_client_hello<S: AsyncRead + Unpin>(
    stream: &mut S,
    mut prefix: Vec<u8>,
    max: usize,
    timeout: Duration,
    origin: &WebsiteOrigin,
) -> Result<Vec<u8>, ProxyError> {
    tokio::time::timeout(timeout, async {
        loop {
            match inspect_records(&prefix, origin)? {
                Inspect::Complete => return Ok(prefix),
                Inspect::Incomplete => {}
            }
            if prefix.len() >= max {
                return Err(ProxyError::TooLarge);
            }
            let mut buffer = [0u8; 4096];
            let limit = (max - prefix.len()).min(buffer.len());
            let count = stream.read(&mut buffer[..limit]).await?;
            if count == 0 {
                return Err(ProxyError::TlsClientHello);
            }
            prefix.extend_from_slice(&buffer[..count]);
        }
    })
    .await
    .map_err(|_| ProxyError::Timeout)?
}

enum Inspect {
    Complete,
    Incomplete,
}

fn inspect_records(prefix: &[u8], origin: &WebsiteOrigin) -> Result<Inspect, ProxyError> {
    let mut offset = 0;
    let mut handshake = Vec::new();
    while offset < prefix.len() {
        if prefix.len() - offset < 5 {
            return Ok(Inspect::Incomplete);
        }
        if prefix[offset] != 22 || prefix[offset + 1] != 3 {
            return Err(ProxyError::TlsClientHello);
        }
        let record_len = usize::from(u16::from_be_bytes([prefix[offset + 3], prefix[offset + 4]]));
        let end = offset + 5 + record_len;
        if end > prefix.len() {
            return Ok(Inspect::Incomplete);
        }
        handshake.extend_from_slice(&prefix[offset + 5..end]);
        if handshake.len() >= 4 {
            if handshake[0] != 1 {
                return Err(ProxyError::TlsClientHello);
            }
            let hello_len = (usize::from(handshake[1]) << 16)
                | (usize::from(handshake[2]) << 8)
                | usize::from(handshake[3]);
            if handshake.len() >= hello_len + 4 {
                validate_hello(&handshake[4..4 + hello_len], origin)?;
                return Ok(Inspect::Complete);
            }
        }
        offset = end;
    }
    Ok(Inspect::Incomplete)
}

fn validate_hello(hello: &[u8], origin: &WebsiteOrigin) -> Result<(), ProxyError> {
    let mut offset = 2 + 32;
    let session_len = *hello.get(offset).ok_or(ProxyError::TlsClientHello)? as usize;
    offset += 1 + session_len;
    let cipher_len = be_u16(hello, offset)? as usize;
    offset += 2 + cipher_len;
    let compression_len = *hello.get(offset).ok_or(ProxyError::TlsClientHello)? as usize;
    offset += 1 + compression_len;
    let extensions_len = be_u16(hello, offset)? as usize;
    offset += 2;
    let extensions = hello
        .get(offset..offset + extensions_len)
        .ok_or(ProxyError::TlsClientHello)?;
    if offset + extensions_len != hello.len() {
        return Err(ProxyError::TlsClientHello);
    }

    let mut cursor = 0;
    let mut sni = None;
    while cursor < extensions.len() {
        let extension_type = be_u16(extensions, cursor)?;
        let len = be_u16(extensions, cursor + 2)? as usize;
        let data = extensions
            .get(cursor + 4..cursor + 4 + len)
            .ok_or(ProxyError::TlsClientHello)?;
        if extension_type == 0xfe0d {
            return Err(ProxyError::TlsEch);
        }
        if extension_type == 0 {
            if sni.is_some() {
                return Err(ProxyError::TlsSni);
            }
            sni = Some(parse_sni(data)?);
        }
        cursor += 4 + len;
    }
    let sni = sni.ok_or(ProxyError::TlsSni)?;
    let normalized = WebsiteOrigin::parse(&format!("https://{sni}:{}", origin.port()))
        .map_err(|_| ProxyError::TlsSni)?;
    if normalized.hostname() != origin.hostname() {
        return Err(ProxyError::TlsSni);
    }
    Ok(())
}

fn parse_sni(data: &[u8]) -> Result<&str, ProxyError> {
    let list_len = be_u16(data, 0)? as usize;
    let list = data
        .get(2..2 + list_len)
        .ok_or(ProxyError::TlsClientHello)?;
    if list.len() < 3 || list[0] != 0 {
        return Err(ProxyError::TlsSni);
    }
    let name_len = be_u16(list, 1)? as usize;
    let name = list.get(3..3 + name_len).ok_or(ProxyError::TlsSni)?;
    if name.len() + 3 != list.len() {
        return Err(ProxyError::TlsSni);
    }
    std::str::from_utf8(name).map_err(|_| ProxyError::TlsSni)
}

fn be_u16(bytes: &[u8], offset: usize) -> Result<u16, ProxyError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or(ProxyError::TlsClientHello)?;
    Ok(u16::from_be_bytes([value[0], value[1]]))
}

#[cfg(test)]
pub(crate) fn test_client_hello(hostname: Option<&str>) -> Vec<u8> {
    test_client_hello_with_ech(hostname, false)
}

#[cfg(test)]
pub(crate) fn test_client_hello_with_ech(hostname: Option<&str>, ech: bool) -> Vec<u8> {
    let mut extensions = Vec::new();
    if let Some(hostname) = hostname {
        let mut sni = Vec::new();
        sni.extend_from_slice(&(hostname.len() as u16 + 3).to_be_bytes());
        sni.push(0);
        sni.extend_from_slice(&(hostname.len() as u16).to_be_bytes());
        sni.extend_from_slice(hostname.as_bytes());
        extensions.extend_from_slice(&0u16.to_be_bytes());
        extensions.extend_from_slice(&(sni.len() as u16).to_be_bytes());
        extensions.extend_from_slice(&sni);
    }
    if ech {
        extensions.extend_from_slice(&0xfe0du16.to_be_bytes());
        extensions.extend_from_slice(&0u16.to_be_bytes());
    }
    let mut hello = Vec::new();
    hello.extend_from_slice(&[3, 3]);
    hello.extend_from_slice(&[0; 32]);
    hello.push(0);
    hello.extend_from_slice(&2u16.to_be_bytes());
    hello.extend_from_slice(&0x1301u16.to_be_bytes());
    hello.push(1);
    hello.push(0);
    hello.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    hello.extend_from_slice(&extensions);

    let mut handshake = vec![1];
    let len = hello.len();
    handshake.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
    handshake.extend_from_slice(&hello);
    tls_record(&handshake)
}

#[cfg(test)]
pub(crate) fn split_client_hello(bytes: &[u8], split: usize) -> Vec<u8> {
    let handshake = &bytes[5..];
    let split = split.min(handshake.len());
    let mut records = tls_record(&handshake[..split]);
    records.extend_from_slice(&tls_record(&handshake[split..]));
    records
}

#[cfg(test)]
pub(crate) fn tls_record(payload: &[u8]) -> Vec<u8> {
    let mut record = vec![22, 3, 3];
    record.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    record.extend_from_slice(payload);
    record
}
