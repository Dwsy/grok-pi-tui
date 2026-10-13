use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use xai_grok_sandbox::{WebsiteAction, WebsiteOrigin, WebsitePolicy};

use super::*;
use crate::error::{ConnectionError, write_committed};
use crate::tls::{split_client_hello, test_client_hello, test_client_hello_with_ech, tls_record};

async fn read_response_head(stream: &mut TcpStream, output: &mut Vec<u8>) {
    while !output.windows(4).any(|window| window == b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        let count = stream.read(&mut byte).await.unwrap();
        if count == 0 {
            break;
        }
        output.push(byte[0]);
    }
}

#[derive(Default)]
struct MockResolver {
    answers: Mutex<BTreeMap<String, io::Result<Vec<SocketAddr>>>>,
}

impl MockResolver {
    fn answer(&self, hostname: &str, addresses: Vec<SocketAddr>) {
        self.answers
            .lock()
            .unwrap()
            .insert(hostname.to_owned(), Ok(addresses));
    }

    fn fail(&self, hostname: &str) {
        self.answers
            .lock()
            .unwrap()
            .insert(hostname.to_owned(), Err(io::Error::other("dns failed")));
    }
}

#[async_trait]
impl Resolver for MockResolver {
    async fn resolve(&self, hostname: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
        self.answers
            .lock()
            .unwrap()
            .remove(hostname)
            .unwrap_or_else(|| Ok(Vec::new()))
    }
}

#[derive(Default)]
struct MockConnector {
    peers: Mutex<VecDeque<BoxProxyIo>>,
    addresses: Mutex<Vec<Vec<SocketAddr>>>,
}

impl MockConnector {
    fn push(&self) -> DuplexStream {
        let (proxy, peer) = tokio::io::duplex(128 * 1024);
        self.peers.lock().unwrap().push_back(Box::new(proxy));
        peer
    }

    fn push_io(&self, io: impl ProxyIo + 'static) {
        self.peers.lock().unwrap().push_back(Box::new(io));
    }

    fn addresses(&self) -> Vec<Vec<SocketAddr>> {
        self.addresses.lock().unwrap().clone()
    }
}

struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

struct HeldIo {
    _drop_signal: DropSignal,
}

impl AsyncRead for HeldIo {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Pending
    }
}

impl AsyncWrite for HeldIo {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[async_trait]
impl Connector for MockConnector {
    async fn connect(&self, addresses: &[SocketAddr]) -> io::Result<BoxProxyIo> {
        self.addresses.lock().unwrap().push(addresses.to_vec());
        self.peers
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| io::Error::other("no mock connection"))
    }
}

struct FailingIo {
    readable: std::io::Cursor<Vec<u8>>,
    write_error: bool,
}

impl FailingIo {
    fn relay_error() -> Self {
        Self {
            readable: std::io::Cursor::new(Vec::new()),
            write_error: false,
        }
    }
}

impl AsyncRead for FailingIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let remaining = self.readable.get_ref().len() - self.readable.position() as usize;
        if remaining == 0 {
            return Poll::Ready(Err(io::Error::other("relay failed")));
        }
        let count = remaining.min(buf.remaining());
        let start = self.readable.position() as usize;
        buf.put_slice(&self.readable.get_ref()[start..start + count]);
        self.readable.set_position((start + count) as u64);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for FailingIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.write_error {
            return Poll::Ready(Err(io::Error::other("relay failed")));
        }
        self.write_error = true;
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct RecordingIo {
    written: Arc<Mutex<Vec<u8>>>,
}

impl AsyncRead for RecordingIo {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for RecordingIo {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.written.lock().unwrap().extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct ShortWriteIo {
    written: Arc<Mutex<Vec<u8>>>,
    failed: bool,
}

impl AsyncRead for ShortWriteIo {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::other("read failed")))
    }
}

impl AsyncWrite for ShortWriteIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.failed {
            return Poll::Ready(Err(io::Error::other("short write failed")));
        }
        self.failed = true;
        let count = bytes.len().min(5);
        self.written
            .lock()
            .unwrap()
            .extend_from_slice(&bytes[..count]);
        Poll::Ready(Ok(count))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct Fixture {
    handle: EgressProxyHandle,
    resolver: Arc<MockResolver>,
    connector: Arc<MockConnector>,
}

impl Fixture {
    async fn start(policy: WebsitePolicy) -> Self {
        let resolver = Arc::new(MockResolver::default());
        let connector = Arc::new(MockConnector::default());
        Self::start_with(policy, resolver, connector).await
    }

    async fn start_with(
        policy: WebsitePolicy,
        resolver: Arc<MockResolver>,
        connector: Arc<MockConnector>,
    ) -> Self {
        let options = EgressProxyOptions {
            resolver: resolver.clone(),
            connector: connector.clone(),
            request_timeout: Duration::from_secs(2),
            dns_timeout: Duration::from_secs(2),
            connect_timeout: Duration::from_secs(2),
            tls_hello_timeout: Duration::from_secs(2),
            drain_timeout: Duration::from_secs(2),
            ..Default::default()
        };
        let handle = EgressProxy::start(policy, options).await.unwrap();
        Self {
            handle,
            resolver,
            connector,
        }
    }

    fn auth(&self) -> String {
        self.handle.proxy_authorization()
    }

    fn addr(&self) -> SocketAddr {
        self.handle.address()
    }
}

fn allow(origins: &[&str]) -> WebsitePolicy {
    WebsitePolicy::new(
        WebsiteAction::Deny,
        origins
            .iter()
            .map(|value| WebsiteOrigin::parse(value).unwrap()),
        [],
    )
}

fn public_v4(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), port)
}

#[derive(Clone, Copy)]
enum ExpectedResponseBody {
    Ordinary,
    Head,
}

async fn send(addr: SocketAddr, request: &[u8]) -> Vec<u8> {
    send_with_body(addr, request, ExpectedResponseBody::Ordinary).await
}

async fn send_head(addr: SocketAddr, request: &[u8]) -> Vec<u8> {
    send_with_body(addr, request, ExpectedResponseBody::Head).await
}

async fn send_with_body(
    addr: SocketAddr,
    request: &[u8],
    expected_body: ExpectedResponseBody,
) -> Vec<u8> {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request).await.unwrap();
    let mut response = Vec::new();
    if let Err(error) = stream.read_to_end(&mut response).await {
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
        assert!(
            complete_http_response(&response, expected_body),
            "incomplete response before reset"
        );
    }
    response
}

fn complete_http_response(response: &[u8], expected_body: ExpectedResponseBody) -> bool {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Response::new(&mut headers);
    let Ok(httparse::Status::Complete(head_end)) = parsed.parse(response) else {
        return false;
    };
    if parsed.version != Some(1) || parsed.code.is_none_or(|status| status < 200) {
        return false;
    }

    let mut content_length = None;
    for header in parsed.headers {
        if header.name.eq_ignore_ascii_case("transfer-encoding") {
            return false;
        }
        if header.name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return false;
            }
            let Ok(value) = std::str::from_utf8(header.value) else {
                return false;
            };
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return false;
            }
            let Ok(value) = value.parse::<usize>() else {
                return false;
            };
            content_length = Some(value);
        }
    }

    let body_len = response.len() - head_end;
    if matches!(expected_body, ExpectedResponseBody::Head) {
        return body_len == 0;
    }
    match parsed.code {
        Some(204) => content_length.is_none() && body_len == 0,
        Some(205) => content_length.is_none_or(|length| length == 0) && body_len == 0,
        Some(304) => body_len == 0,
        Some(_) => content_length.is_some_and(|length| length == body_len),
        None => false,
    }
}

#[test]
fn response_completeness_respects_method_status_and_framing() {
    let ordinary = ExpectedResponseBody::Ordinary;
    let head = ExpectedResponseBody::Head;
    assert!(complete_http_response(
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc",
        ordinary
    ));
    assert!(!complete_http_response(
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nab",
        ordinary
    ));
    assert!(complete_http_response(
        b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n",
        head
    ));
    assert!(!complete_http_response(
        b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nx",
        head
    ));
    assert!(!complete_http_response(
        b"HTTP/1.1 200 OK\r\n\r\n",
        ordinary
    ));
    assert!(complete_http_response(
        b"HTTP/1.1 204 No Content\r\n\r\n",
        ordinary
    ));
    assert!(!complete_http_response(
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n",
        ordinary
    ));
    for response in [
        b"HTTP/1.1 205 Reset Content\r\n\r\n".as_slice(),
        b"HTTP/1.1 205 Reset Content\r\nContent-Length: 0\r\n\r\n".as_slice(),
    ] {
        assert!(complete_http_response(response, ordinary));
    }
    assert!(!complete_http_response(
        b"HTTP/1.1 205 Reset Content\r\nContent-Length: 1\r\n\r\n",
        ordinary
    ));
    assert!(complete_http_response(
        b"HTTP/1.1 304 Not Modified\r\n\r\n",
        ordinary
    ));
    assert!(complete_http_response(
        b"HTTP/1.1 304 Not Modified\r\nContent-Length: 12\r\n\r\n",
        ordinary
    ));
    for response in [
        b"HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n".as_slice(),
        b"HTTP/1.1 100 Continue\r\nContent-Length: 0\r\n\r\n".as_slice(),
    ] {
        assert!(!complete_http_response(response, ordinary));
    }
}

async fn wait_for_tracked_tasks(handle: &EgressProxyHandle, expected: usize) {
    for _ in 0..100 {
        if handle.tracked_tasks() == expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(handle.tracked_tasks(), expected);
}

fn status(response: &[u8]) -> u16 {
    let line = response.split(|byte| *byte == b'\n').next().unwrap();
    std::str::from_utf8(line)
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

async fn http_upstream(mut peer: DuplexStream, expected_header_absent: &[&str]) -> JoinHandle<()> {
    let absent = expected_header_absent
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    tokio::spawn(async move {
        let mut request = Vec::new();
        loop {
            let mut chunk = [0u8; 4096];
            let count = peer.read(&mut chunk).await.unwrap();
            if count == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..count]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let lower = String::from_utf8_lossy(&request).to_ascii_lowercase();
        for name in absent {
            assert!(!lower.contains(&format!("{name}:")), "{lower}");
        }
        peer.write_all(
            b"HTTP/1.1 302 Found\r\nLocation: https://other.example/next\r\nProxy-Authentication-Info: nextnonce=origin\r\nContent-Length: 0\r\n\r\n",
        )
        .await
        .unwrap();
    })
}

mod connect;
mod decider;
mod hold_budget;
mod http;
mod lifecycle;
mod request;
