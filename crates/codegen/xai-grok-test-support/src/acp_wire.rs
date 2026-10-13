//! The raw JSON-RPC lines a test client reads and writes on stdio.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// One line without its trailing newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireLine {
    FromAgent(String),
    FromClient(String),
}

/// The lines of one connection in the order the client read or wrote them.
#[derive(Clone, Default)]
pub(crate) struct Wire {
    lines: Arc<Mutex<Vec<WireLine>>>,
}

impl Wire {
    pub(crate) fn lines(&self) -> Vec<WireLine> {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<WireLine>> {
        self.lines.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn record_agent_lines<R>(&self, reader: R) -> ReadTap<R> {
        ReadTap {
            inner: reader,
            lines: LineRecorder::new(self.clone(), WireLine::FromAgent),
        }
    }

    pub(crate) fn record_client_lines<W>(&self, writer: W) -> WriteTap<W> {
        WriteTap {
            inner: writer,
            lines: LineRecorder::new(self.clone(), WireLine::FromClient),
        }
    }

    fn push(&self, line: WireLine) {
        self.lock().push(line);
    }
}

/// Buffers a partial line until its newline arrives. A line split across reads is recorded once.
struct LineRecorder {
    wire: Wire,
    pending: Vec<u8>,
    tag: fn(String) -> WireLine,
}

impl LineRecorder {
    fn new(wire: Wire, tag: fn(String) -> WireLine) -> Self {
        LineRecorder {
            wire,
            pending: Vec::new(),
            tag,
        }
    }

    fn record(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        while let Some(newline) = self.pending.iter().position(|&byte| byte == b'\n') {
            let mut line: Vec<u8> = self.pending.drain(..=newline).collect();
            line.pop();
            self.wire
                .push((self.tag)(String::from_utf8_lossy(&line).into_owned()));
        }
    }
}

pub(crate) struct ReadTap<R> {
    inner: R,
    lines: LineRecorder,
}

impl<R: AsyncRead + Unpin> AsyncRead for ReadTap<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let filled_before = buf.filled().len();
        let polled = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            this.lines
                .record(buf.filled().get(filled_before..).unwrap_or_default());
        }
        polled
    }
}

pub(crate) struct WriteTap<W> {
    inner: W,
    lines: LineRecorder,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for WriteTap<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(written)) = polled {
            this.lines.record(buf.get(..written).unwrap_or_default());
        }
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
#[path = "acp_wire_tests.rs"]
mod tests;
