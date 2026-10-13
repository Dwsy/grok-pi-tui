//! Loopback server that stores the HTTP bodies posted to it.
//! Sentry crash reports arrive as envelopes on whatever path the DSN names.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// A loopback HTTP server and the bodies it has accepted.
pub struct EnvelopeSink {
    origin: String,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl EnvelopeSink {
    /// Binds `127.0.0.1` on an ephemeral port and answers every request `200`.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("envelope sink binds");
        let origin = format!("http://{}", listener.local_addr().expect("bound addr"));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&bodies);
        thread::spawn(move || serve(listener, recorded));
        Self { origin, bodies }
    }

    /// A Sentry DSN whose envelope posts land on this sink.
    #[must_use]
    pub fn dsn(&self) -> String {
        let host = self.origin.trim_start_matches("http://");
        format!("http://public@{host}/1")
    }

    /// Bodies received so far, in arrival order.
    #[must_use]
    pub fn bodies(&self) -> Vec<String> {
        self.bodies
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

fn serve(listener: TcpListener, bodies: Arc<Mutex<Vec<String>>>) {
    let _ = listener.set_nonblocking(false);
    for stream in listener.incoming().flatten() {
        let _ = read_body(stream, &bodies);
    }
}

fn read_body(mut stream: TcpStream, bodies: &Mutex<Vec<String>>) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(buf.get(..head_end).unwrap_or(&buf)).to_string();
    let body_len = content_length(&head);
    while buf.len() < head_end + body_len {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
    }
    let body = buf
        .get(head_end..)
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default();
    bodies
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(body);
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
    stream.write_all(response)?;
    Ok(())
}

fn content_length(head: &str) -> usize {
    head.lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse().ok())
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use super::EnvelopeSink;

    #[test]
    fn records_the_posted_body() {
        let sink = EnvelopeSink::start();
        let host = sink.origin.trim_start_matches("http://");
        let mut stream = TcpStream::connect(host).expect("connect");
        let body = "envelope-marker";
        let request = format!(
            "POST /api/1/envelope/ HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("write");
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        assert_eq!(sink.bodies(), vec![body.to_owned()]);
    }
}
