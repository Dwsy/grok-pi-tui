use super::*;

#[tokio::test]
async fn upstream_request_write_failure_records_connect_failed_only() {
    let fixture = Fixture::start(allow(&["http://allowed.example"])).await;
    fixture
        .resolver
        .answer("allowed.example", vec![public_v4(80)]);
    fixture.connector.push_io(ShortWriteIo {
        written: Arc::new(Mutex::new(Vec::new())),
        failed: true,
    });
    let request = format!(
        "GET http://allowed.example/ HTTP/1.1\r\nHost: allowed.example\r\nProxy-Authorization: {}\r\n\r\n",
        fixture.auth()
    );
    assert_eq!(status(&send(fixture.addr(), request.as_bytes()).await), 502);
    let metrics = fixture.handle.metrics();
    assert_eq!(metrics.connect_failed, 1);
    assert_eq!(metrics.malformed, 0);
    fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn precommit_http_response_failure_records_one_terminal_outcome() {
    let fixture = Fixture::start(allow(&["http://allowed.example"])).await;
    fixture
        .resolver
        .answer("allowed.example", vec![public_v4(80)]);
    fixture.connector.push_io(FailingIo::relay_error());
    let request = format!(
        "GET http://allowed.example/ HTTP/1.1\r\nHost: allowed.example\r\nProxy-Authorization: {}\r\n\r\n",
        fixture.auth()
    );
    let response = send(fixture.addr(), request.as_bytes()).await;
    assert_eq!(status(&response), 502);
    let metrics = fixture.handle.metrics();
    assert_eq!(metrics.ok, 0);
    assert_eq!(metrics.connect_failed, 1);
    assert_eq!(metrics.malformed, 0);
    fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn partial_http_response_failure_never_appends_proxy_error() {
    let fixture = Fixture::start(allow(&["http://allowed.example"])).await;
    fixture
        .resolver
        .answer("allowed.example", vec![public_v4(80)]);
    let mut peer = fixture.connector.push();
    let upstream = tokio::spawn(async move {
        let mut request = Vec::new();
        loop {
            let mut chunk = [0u8; 1024];
            let count = peer.read(&mut chunk).await.unwrap();
            request.extend_from_slice(&chunk[..count]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        peer.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc")
            .await
            .unwrap();
    });
    let request = format!(
        "GET http://allowed.example/ HTTP/1.1\r\nHost: allowed.example\r\nProxy-Authorization: {}\r\n\r\n",
        fixture.auth()
    );
    let response = send(fixture.addr(), request.as_bytes()).await;
    let text = String::from_utf8_lossy(&response);
    assert_eq!(text.matches("HTTP/1.1").count(), 1);
    assert!(text.ends_with("abc"));
    let metrics = fixture.handle.metrics();
    assert_eq!(metrics.ok, 0);
    assert_eq!(metrics.connect_failed, 1);
    assert_eq!(metrics.malformed, 0);
    upstream.await.unwrap();
    fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn response_framing_handles_head_204_205_and_304() {
    for (method, upstream_response, expected_length) in [
        ("HEAD", "HTTP/1.1 200 OK\r\n\r\n", None),
        (
            "HEAD",
            "HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\n",
            Some(17),
        ),
        ("GET", "HTTP/1.1 204 No Content\r\n\r\n", None),
        ("GET", "HTTP/1.1 205 Reset Content\r\n\r\n", None),
        (
            "GET",
            "HTTP/1.1 205 Reset Content\r\nContent-Length: 0\r\n\r\n",
            None,
        ),
        (
            "GET",
            "HTTP/1.1 304 Not Modified\r\nContent-Length: 23\r\n\r\n",
            Some(23),
        ),
    ] {
        let fixture = Fixture::start(allow(&["http://allowed.example"])).await;
        fixture
            .resolver
            .answer("allowed.example", vec![public_v4(80)]);
        let mut peer = fixture.connector.push();
        let response = upstream_response.as_bytes().to_vec();
        let upstream = tokio::spawn(async move {
            let mut request = Vec::new();
            loop {
                let mut chunk = [0u8; 1024];
                let count = peer.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            peer.write_all(&response).await.unwrap();
        });
        let request = format!(
            "{method} http://allowed.example/ HTTP/1.1\r\nHost: allowed.example\r\nProxy-Authorization: {}\r\n\r\n",
            fixture.auth()
        );
        let response = if method == "HEAD" {
            send_head(fixture.addr(), request.as_bytes()).await
        } else {
            send(fixture.addr(), request.as_bytes()).await
        };
        assert!(matches!(status(&response), 200 | 204 | 205 | 304));
        let text = String::from_utf8_lossy(&response);
        match expected_length {
            Some(length) => assert!(text.contains(&format!("Content-Length: {length}\r\n"))),
            None => assert!(!text.contains("Content-Length:")),
        }
        assert!(response.ends_with(b"\r\n\r\n"));
        assert_eq!(fixture.handle.metrics().ok, 1);
        upstream.await.unwrap();
        fixture.handle.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn response_205_rejects_nonzero_or_buffered_body() {
    for upstream_response in [
        "HTTP/1.1 205 Reset Content\r\nContent-Length: 1\r\n\r\n",
        "HTTP/1.1 205 Reset Content\r\n\r\nx",
    ] {
        let fixture = Fixture::start(allow(&["http://allowed.example"])).await;
        fixture
            .resolver
            .answer("allowed.example", vec![public_v4(80)]);
        let mut peer = fixture.connector.push();
        let nonzero_length = upstream_response.contains("Content-Length: 1");
        let upstream_response = upstream_response.as_bytes().to_vec();
        let upstream = tokio::spawn(async move {
            let mut request = Vec::new();
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let mut chunk = [0u8; 1024];
                let count = peer.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..count]);
            }
            peer.write_all(&upstream_response).await.unwrap();
        });
        let request = format!(
            "GET http://allowed.example/ HTTP/1.1\r\nHost: allowed.example\r\nProxy-Authorization: {}\r\n\r\n",
            fixture.auth()
        );
        let response = send(fixture.addr(), request.as_bytes()).await;
        let text = String::from_utf8_lossy(&response);
        if nonzero_length {
            assert_eq!(status(&response), 502);
        } else {
            assert_eq!(status(&response), 205);
            assert_eq!(text.matches("HTTP/1.1").count(), 1);
            assert!(!text.ends_with('x'));
        }
        let metrics = fixture.handle.metrics();
        assert_eq!(metrics.connect_failed, 1);
        assert_eq!(metrics.malformed, 0);
        upstream.await.unwrap();
        fixture.handle.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn committed_write_marks_http_and_connect_short_write_failures() {
    for response in [
        b"HTTP/1.1 200 OK\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 Connection Established\r\n\r\n".as_slice(),
    ] {
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut writer = ShortWriteIo {
            written: written.clone(),
            failed: false,
        };
        let error = write_committed(&mut writer, response).await.unwrap_err();
        assert!(matches!(error, ConnectionError::Committed(_)));
        assert_eq!(written.lock().unwrap().as_slice(), b"HTTP/");
    }
}

#[tokio::test]
async fn resolution_rejects_empty_private_mixed_and_uses_pinned_public_set() {
    let fixture = Fixture::start(allow(&["http://allowed.example"])).await;
    let auth = fixture.auth();
    let request = || {
        format!(
            "GET http://allowed.example/ HTTP/1.1\r\nHost: allowed.example\r\nProxy-Authorization: {auth}\r\n\r\n"
        )
    };

    fixture.resolver.answer("allowed.example", Vec::new());
    assert_eq!(
        status(&send(fixture.addr(), request().as_bytes()).await),
        502
    );
    fixture.resolver.fail("allowed.example");
    assert_eq!(
        status(&send(fixture.addr(), request().as_bytes()).await),
        502
    );
    fixture.resolver.answer(
        "allowed.example",
        vec![SocketAddr::from(([127, 0, 0, 1], 80))],
    );
    assert_eq!(
        status(&send(fixture.addr(), request().as_bytes()).await),
        403
    );
    fixture.resolver.answer(
        "allowed.example",
        vec![public_v4(80), SocketAddr::from(([10, 0, 0, 1], 80))],
    );
    assert_eq!(
        status(&send(fixture.addr(), request().as_bytes()).await),
        403
    );

    let pinned = vec![
        public_v4(80),
        SocketAddr::new(
            IpAddr::V6("2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap()),
            80,
        ),
    ];
    fixture.resolver.answer("allowed.example", pinned.clone());
    let peer = fixture.connector.push();
    let upstream = http_upstream(peer, &[]).await;
    assert_eq!(
        status(&send(fixture.addr(), request().as_bytes()).await),
        302
    );
    upstream.await.unwrap();
    assert_eq!(fixture.connector.addresses().last(), Some(&pinned));
    fixture.handle.shutdown().await.unwrap();
}
