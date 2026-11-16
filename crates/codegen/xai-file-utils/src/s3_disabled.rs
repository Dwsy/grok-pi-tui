//! Pi-only boundary: stock S3 functions are unavailable without the AWS SDK.
//!
//! Call sites in the shared cloud upload router and stock video tool still
//! compile, but no S3 request is performed in the Pi-only binary.

use std::path::Path;
use std::time::Duration;

/// The stock video tool's credentials type. Debug output never reveals keys.
#[derive(Clone)]
pub struct S3StaticCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl std::fmt::Debug for S3StaticCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3StaticCredentials")
            .field("access_key_id", &"[redacted]")
            .field("secret_access_key", &"[redacted]")
            .finish()
    }
}

fn unavailable() -> anyhow::Error {
    anyhow::anyhow!("Stock S3 support is not included in the Pi-only build")
}

pub async fn upload_bytes(
    _bucket: &str,
    _object_path: &str,
    _content: &[u8],
    _content_type: &str,
    _region: &str,
    _credentials_content: Option<&str>,
    _credentials_file: Option<&str>,
    _endpoint_url: Option<&str>,
) -> anyhow::Result<String> {
    Err(unavailable())
}

pub async fn upload_file(
    _bucket: &str,
    _object_path: &str,
    _file_path: &Path,
    _content_type: &str,
    _region: &str,
    _credentials_content: Option<&str>,
    _credentials_file: Option<&str>,
    _endpoint_url: Option<&str>,
) -> anyhow::Result<String> {
    Err(unavailable())
}

pub async fn upload_stream<R: tokio::io::AsyncRead + Send + Sync + 'static>(
    _bucket: &str,
    _object_path: &str,
    _reader: R,
    _content_type: &str,
    _region: &str,
    _credentials_content: Option<&str>,
    _credentials_file: Option<&str>,
    _endpoint_url: Option<&str>,
) -> anyhow::Result<String> {
    Err(unavailable())
}

pub async fn presign_put_url(
    _region: &str,
    _endpoint_url: Option<&str>,
    _creds: &S3StaticCredentials,
    _bucket: &str,
    _key: &str,
    _content_type: &str,
    _expires_in: Duration,
) -> anyhow::Result<String> {
    Err(unavailable())
}

pub async fn presign_get_url(
    _region: &str,
    _endpoint_url: Option<&str>,
    _creds: &S3StaticCredentials,
    _bucket: &str,
    _key: &str,
    _expires_in: Duration,
) -> anyhow::Result<String> {
    Err(unavailable())
}
