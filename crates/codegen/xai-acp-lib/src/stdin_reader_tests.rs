use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use tokio::io::AsyncReadExt;

use super::*;

const FRAME: &[u8] = b"Content-Length: 2\r\n\r\n{}";

#[tokio::test]
async fn bytes_arrive_while_the_writer_keeps_its_end_open() {
    let (mut writer, reader) = UnixStream::pair().expect("create a socket pair");
    let mut bytes = read_on_thread(reader).expect("spawn the reader thread");
    writer.write_all(FRAME).expect("write a frame");

    let mut frame = vec![0; FRAME.len()];
    tokio::time::timeout(Duration::from_secs(30), bytes.read_exact(&mut frame))
        .await
        .expect("the frame arrives before the writer closes")
        .expect("read the frame");

    assert_eq!(FRAME, frame.as_slice());
}
