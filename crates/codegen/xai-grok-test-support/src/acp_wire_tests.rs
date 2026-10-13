use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::{Wire, WireLine};

#[tokio::test]
async fn split_line_is_recorded_once() {
    let wire = Wire::default();
    let mut reader =
        wire.record_agent_lines(&b"{\"id\":1,\"result\":42.0}\n{\"method\":\"m\"}\npartial"[..]);
    let mut chunk = [0_u8; 5];

    while reader.read(&mut chunk).await.expect("a slice reads") > 0 {}

    assert_eq!(
        vec![
            WireLine::FromAgent("{\"id\":1,\"result\":42.0}".to_owned()),
            WireLine::FromAgent("{\"method\":\"m\"}".to_owned()),
        ],
        wire.lines()
    );
}

#[tokio::test]
async fn reads_and_writes_keep_their_order() {
    let wire = Wire::default();
    let mut reader = wire.record_agent_lines(&b"{\"id\":0,\"method\":\"ask\"}\n"[..]);
    let mut writer = wire.record_client_lines(Vec::new());

    reader
        .read_to_end(&mut Vec::new())
        .await
        .expect("a slice reads");
    writer
        .write_all(b"{\"id\":0,")
        .await
        .expect("a vec accepts writes");
    writer
        .write_all(b"\"result\":null}\n")
        .await
        .expect("a vec accepts writes");

    assert_eq!(
        vec![
            WireLine::FromAgent("{\"id\":0,\"method\":\"ask\"}".to_owned()),
            WireLine::FromClient("{\"id\":0,\"result\":null}".to_owned()),
        ],
        wire.lines()
    );
}
