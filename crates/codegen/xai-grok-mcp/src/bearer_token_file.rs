//! `bearer_token_file` for HTTP MCP servers: the bearer token is read from disk on every request.
//! Another process can therefore rotate the token without the session reconnecting. A per-request
//! read needs no watcher task, sees atomic renames and in-place rewrites alike, and works on
//! network mounts where change notifications are unreliable.
//! Errors name the path but never the file contents.

use std::io::{self, Read};
use std::sync::Arc;
use std::time::Duration;

use http::HeaderValue;
use tokio::task::JoinHandle;
use xai_grok_config::BearerTokenPath;

/// Tokens are short; anything larger is the wrong file.
const MAX_TOKEN_FILE_BYTES: u64 = 16 * 1024;
/// Bounds a read from a stalled network filesystem.
const TOKEN_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Clones share the stalled-read slot, and one value lives for a client's lifetime (reconnects
/// included), so a hung mount pins at most one blocking-pool thread per server.
#[derive(Debug, Clone)]
pub struct BearerTokenFile {
    path: Arc<BearerTokenPath>,
    read_timeout: Duration,
    /// A read that outlived `read_timeout`. Tokio cannot cancel a blocking read, so later reads
    /// fail fast until it returns.
    stalled_read: Arc<parking_lot::Mutex<Option<JoinHandle<io::Result<String>>>>>,
}

impl BearerTokenFile {
    pub fn new(path: BearerTokenPath) -> Self {
        BearerTokenFile {
            path: Arc::new(path),
            read_timeout: TOKEN_READ_TIMEOUT,
            stalled_read: Arc::default(),
        }
    }

    /// Returns the file contents with surrounding whitespace trimmed.
    ///
    /// # Errors
    ///
    /// `TimedOut` when this read or an earlier unreturned one exceeded the read timeout,
    /// `InvalidData` when the file is empty, not UTF-8, larger than [`MAX_TOKEN_FILE_BYTES`], or not
    /// a valid HTTP header value, and the underlying error when the file cannot be opened or read.
    pub(crate) async fn read(&self) -> io::Result<String> {
        let path = &self.path;
        if self
            .stalled_read
            .lock()
            .as_ref()
            .is_some_and(|read| !read.is_finished())
        {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("an earlier read of bearer_token_file {path} has not returned"),
            ));
        }
        let mut read = tokio::task::spawn_blocking({
            let path = Arc::clone(path);
            move || read_token_file_sync(&path)
        });
        let contents = match tokio::time::timeout(self.read_timeout, &mut read).await {
            Ok(joined) => joined.map_err(io::Error::other)?,
            Err(_) => {
                *self.stalled_read.lock() = Some(read);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("reading bearer_token_file {path} timed out"),
                ));
            }
        }
        .map_err(|e| io::Error::new(e.kind(), format!("bearer_token_file {path}: {e}")))?;
        let token = contents.trim();
        if token.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bearer_token_file {path} is empty"),
            ));
        }
        if HeaderValue::from_str(&format!("Bearer {token}")).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bearer_token_file {path} holds characters not allowed in an HTTP header"),
            ));
        }
        Ok(token.to_owned())
    }
}

/// One blocking-pool job per read: open, capped read, and UTF-8 check together.
fn read_token_file_sync(path: &BearerTokenPath) -> io::Result<String> {
    let mut contents = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_TOKEN_FILE_BYTES + 1)
        .read_to_end(&mut contents)?;
    if u64::try_from(contents.len()).unwrap_or(u64::MAX) > MAX_TOKEN_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("larger than {MAX_TOKEN_FILE_BYTES} bytes"),
        ));
    }
    String::from_utf8(contents).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))
}

#[cfg(test)]
#[path = "bearer_token_file_tests.rs"]
mod tests;
