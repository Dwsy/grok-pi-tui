use std::path::{Path, PathBuf};

use super::*;

fn token_file_at(path: &Path) -> BearerTokenFile {
    BearerTokenFile::new(BearerTokenPath::try_from(path.to_path_buf()).expect("absolute path"))
}

fn token_file(contents: &[u8]) -> (tempfile::TempDir, BearerTokenFile) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("token");
    std::fs::write(&path, contents).expect("write token");
    let file = token_file_at(&path);
    (dir, file)
}

async fn invalid_data_message(contents: &[u8]) -> String {
    let (_dir, file) = token_file(contents);
    let err = file.read().await.expect_err("invalid token file");
    assert_eq!(io::ErrorKind::InvalidData, err.kind(), "{err}");
    err.to_string()
}

#[tokio::test]
async fn blank_file_is_invalid_data() {
    let message = invalid_data_message(b" \n").await;
    assert!(message.ends_with("is empty"), "{message}");
}

#[tokio::test]
async fn oversized_file_is_invalid_data_without_echoing_contents() {
    let contents = vec![b'a'; usize::try_from(MAX_TOKEN_FILE_BYTES).expect("fits") + 1];
    let message = invalid_data_message(&contents).await;
    assert!(message.contains("larger than"), "{message}");
    assert!(!message.contains("aaaa"), "{message}");
}

#[tokio::test]
async fn non_utf8_file_is_invalid_data() {
    let message = invalid_data_message(b"tok\xff").await;
    assert!(message.ends_with("not UTF-8"), "{message}");
}

#[tokio::test]
async fn inner_control_byte_is_invalid_data_without_echoing_contents() {
    let message = invalid_data_message(b"secret\nsecond-line").await;
    assert!(
        message.contains("not allowed in an HTTP header"),
        "{message}"
    );
    assert!(!message.contains("secret"), "{message}");
}

#[tokio::test]
async fn missing_file_names_the_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("absent");
    let err = token_file_at(&path)
        .read()
        .await
        .expect_err("missing token file");
    assert_eq!(io::ErrorKind::NotFound, err.kind());
    assert!(
        err.to_string().contains(&path.display().to_string()),
        "{err}"
    );
}

/// Opening a FIFO blocks until a writer appears, which stands in for a hung network mount.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn stalled_read_makes_later_reads_fail_fast() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fifo: PathBuf = dir.path().join("token");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).expect("mkfifo");
    let file = BearerTokenFile {
        read_timeout: Duration::from_millis(50),
        ..token_file_at(&fifo)
    };

    let first = file.read().await.expect_err("the first read stalls");
    assert_eq!(io::ErrorKind::TimedOut, first.kind());
    assert!(first.to_string().ends_with("timed out"), "{first}");

    let second = file
        .clone()
        .read()
        .await
        .expect_err("a later read fails fast");
    assert_eq!(io::ErrorKind::TimedOut, second.kind());
    assert!(second.to_string().ends_with("has not returned"), "{second}");

    // Release the blocked thread so it does not outlive the test.
    drop(
        std::fs::OpenOptions::new()
            .write(true)
            .open(&fifo)
            .expect("open fifo writer"),
    );
}
