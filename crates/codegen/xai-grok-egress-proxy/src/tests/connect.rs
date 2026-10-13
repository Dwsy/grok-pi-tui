use super::*;

#[tokio::test]
async fn exact_policy_denies_wrong_scheme_port_and_subdomain() {
    let fixture = Fixture::start(allow(&["http://example.com"])).await;
    for target in [
        "http://sub.example.com/",
        "http://example.com:8080/",
        "https://example.com/",
    ] {
        let host = target
            .strip_prefix("http://")
            .or_else(|| target.strip_prefix("https://"))
            .unwrap()
            .trim_end_matches('/');
        let request = format!(
            "GET {target} HTTP/1.1\r\nHost: {host}\r\nProxy-Authorization: {}\r\n\r\n",
            fixture.auth()
        );
        let expected = if target.starts_with("https://") {
            400
        } else {
            403
        };
        assert_eq!(
            status(&send(fixture.addr(), request.as_bytes()).await),
            expected
        );
    }
    fixture.handle.shutdown().await.unwrap();
}

async fn connect_request_with_auth_and_host(
    fixture: &Fixture,
    authority: &str,
    host: &str,
    hello: &[u8],
    auth: Option<&str>,
) -> Vec<u8> {
    let mut stream = TcpStream::connect(fixture.addr()).await.unwrap();
    let auth = auth
        .map(|value| format!("Proxy-Authorization: {value}\r\n"))
        .unwrap_or_default();
    let request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {host}\r\n{auth}\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    read_response_head(&mut stream, &mut response).await;
    if status(&response) == 200 {
        stream.write_all(hello).await.unwrap();
        stream.shutdown().await.unwrap();
        stream.read_to_end(&mut response).await.unwrap();
    }
    response
}

async fn connect_request_with_auth(
    fixture: &Fixture,
    authority: &str,
    hello: &[u8],
    auth: Option<&str>,
) -> Vec<u8> {
    connect_request_with_auth_and_host(fixture, authority, authority, hello, auth).await
}

async fn connect_request(fixture: &Fixture, authority: &str, hello: &[u8]) -> Vec<u8> {
    connect_request_with_auth(fixture, authority, hello, Some(&fixture.auth())).await
}

async fn assert_tls_denied_without_forwarding(hello: Vec<u8>) {
    let fixture = Fixture::start(allow(&["https://allowed.example"])).await;
    fixture
        .resolver
        .answer("allowed.example", vec![public_v4(443)]);
    let written = Arc::new(Mutex::new(Vec::new()));
    fixture.connector.push_io(RecordingIo {
        written: written.clone(),
    });

    let response = connect_request(&fixture, "allowed.example:443", &hello).await;
    let text = String::from_utf8_lossy(&response);
    assert_eq!(status(&response), 200);
    assert_eq!(text.matches("HTTP/1.1").count(), 1);
    assert!(!text.contains("denied"));
    assert!(written.lock().unwrap().is_empty());
    let metrics = fixture.handle.metrics();
    assert_eq!(metrics.tls_denied, 1);
    assert_eq!(metrics.ok, 0);
    assert_eq!(
        metrics.ok
            + metrics.malformed
            + metrics.unauthenticated
            + metrics.policy_denied
            + metrics.address_denied
            + metrics.dns_failed
            + metrics.connect_failed
            + metrics.timeout
            + metrics.overloaded
            + metrics.tls_denied,
        1
    );
    fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_sends_200_before_waiting_for_client_hello() {
    let fixture = Fixture::start(allow(&["https://allowed.example"])).await;
    fixture
        .resolver
        .answer("allowed.example", vec![public_v4(443)]);
    let mut peer = fixture.connector.push();
    let mut stream = TcpStream::connect(fixture.addr()).await.unwrap();
    let request = format!(
        "CONNECT allowed.example:443 HTTP/1.1\r\nHost: allowed.example:443\r\nProxy-Authorization: {}\r\n\r\n",
        fixture.auth()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    read_response_head(&mut stream, &mut response).await;
    assert_eq!(status(&response), 200);
    let hello = test_client_hello(Some("allowed.example"));
    stream.write_all(&hello).await.unwrap();
    let mut received = vec![0; hello.len()];
    peer.read_exact(&mut received).await.unwrap();
    assert_eq!(received, hello);
    drop(peer);
    let _ = stream.read_to_end(&mut response).await;
    drop(stream);
    wait_for_tracked_tasks(&fixture.handle, 0).await;
    fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_requires_matching_tls_sni_and_preserves_hello() {
    let fixture = Fixture::start(allow(&["https://allowed.example"])).await;
    let hello = test_client_hello(Some("allowed.example"));
    assert_eq!(
        status(&connect_request_with_auth(&fixture, "allowed.example:443", &hello, None).await),
        407
    );
    assert_eq!(
        status(
            &connect_request_with_auth(
                &fixture,
                "allowed.example:443",
                &hello,
                Some("Bearer wrong")
            )
            .await
        ),
        407
    );
    fixture
        .resolver
        .answer("allowed.example", vec![public_v4(443)]);
    let mut peer = fixture.connector.push();
    let expected = hello.clone();
    let read = tokio::spawn(async move {
        let mut received = vec![0; expected.len()];
        peer.read_exact(&mut received).await.unwrap();
        assert_eq!(received, expected);
        peer.write_all(b"upstream").await.unwrap();
        peer.shutdown().await.unwrap();
    });
    let response = connect_request(&fixture, "allowed.example:443", &hello).await;
    assert_eq!(status(&response), 200);
    assert!(response.ends_with(b"upstream"));
    assert_eq!(fixture.connector.addresses(), vec![vec![public_v4(443)]]);
    read.await.unwrap();
    wait_for_tracked_tasks(&fixture.handle, 0).await;
    assert_eq!(fixture.handle.metrics().ok, 1);

    for hello in [
        test_client_hello(Some("other.example")),
        test_client_hello(None),
        b"not tls".to_vec(),
    ] {
        assert_tls_denied_without_forwarding(hello).await;
    }
    assert_eq!(fixture.handle.metrics().ok, 1);

    let denied = Fixture::start(allow(&["https://allowed.example:8443"])).await;
    let response = connect_request(
        &denied,
        "allowed.example:443",
        &test_client_hello(Some("allowed.example")),
    )
    .await;
    assert_eq!(status(&response), 403);
    let subdomain = connect_request(
        &denied,
        "sub.allowed.example:8443",
        &test_client_hello(Some("sub.allowed.example")),
    )
    .await;
    assert_eq!(status(&subdomain), 403);
    assert!(denied.connector.addresses().is_empty());
    fixture.handle.shutdown().await.unwrap();
    denied.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_authority_and_host_compare_canonically() {
    for (authority, host, policy) in [
        ("Example.COM:443", "example.com:443", "https://example.com"),
        ("example.com.:443", "EXAMPLE.com:443", "https://example.com"),
        (
            "bücher.example:443",
            "xn--bcher-kva.example:443",
            "https://xn--bcher-kva.example",
        ),
    ] {
        let fixture = Fixture::start(allow(&[policy])).await;
        let resolved_host = if policy.contains("xn--") {
            "xn--bcher-kva.example"
        } else {
            "example.com"
        };
        fixture.resolver.answer(resolved_host, vec![public_v4(443)]);
        let mut peer = fixture.connector.push();
        let sni = if policy.contains("xn--") {
            "xn--bcher-kva.example"
        } else {
            "example.com"
        };
        let hello = test_client_hello(Some(sni));
        let expected = hello.clone();
        let upstream = tokio::spawn(async move {
            let mut received = vec![0; expected.len()];
            peer.read_exact(&mut received).await.unwrap();
            peer.shutdown().await.unwrap();
        });
        let response = connect_request_with_auth_and_host(
            &fixture,
            authority,
            host,
            &hello,
            Some(&fixture.auth()),
        )
        .await;
        assert_eq!(status(&response), 200);
        upstream.await.unwrap();
        fixture.handle.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn post_connect_200_failure_never_appends_plaintext_error() {
    let fixture = Fixture::start(allow(&["https://allowed.example"])).await;
    fixture
        .resolver
        .answer("allowed.example", vec![public_v4(443)]);
    fixture.connector.push_io(FailingIo::relay_error());
    let response = connect_request(
        &fixture,
        "allowed.example:443",
        &test_client_hello(Some("allowed.example")),
    )
    .await;
    let text = String::from_utf8_lossy(&response);
    assert_eq!(text.matches("HTTP/1.1").count(), 1);
    assert!(!text.contains("upstream_failure"));
    let metrics = fixture.handle.metrics();
    assert_eq!(metrics.ok, 0);
    assert_eq!(metrics.connect_failed, 1);
    assert_eq!(metrics.malformed, 0);
    fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn fragmented_and_coalesced_client_hello_bytes_are_preserved() {
    for prefix in {
        let hello = test_client_hello(Some("allowed.example"));
        let fragmented = split_client_hello(&hello, 12);
        let mut coalesced = hello;
        coalesced.extend_from_slice(&tls_record(b"following"));
        [fragmented, coalesced]
    } {
        let fixture = Fixture::start(allow(&["https://allowed.example"])).await;
        fixture
            .resolver
            .answer("allowed.example", vec![public_v4(443)]);
        let mut peer = fixture.connector.push();
        let expected = prefix.clone();
        let upstream = tokio::spawn(async move {
            let mut received = vec![0; expected.len()];
            peer.read_exact(&mut received).await.unwrap();
            assert_eq!(received, expected);
            peer.shutdown().await.unwrap();
        });
        assert_eq!(
            status(&connect_request(&fixture, "allowed.example:443", &prefix).await),
            200
        );
        upstream.await.unwrap();
        fixture.handle.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn connect_rejects_ech() {
    assert_tls_denied_without_forwarding(test_client_hello_with_ech(Some("allowed.example"), true))
        .await;
}
