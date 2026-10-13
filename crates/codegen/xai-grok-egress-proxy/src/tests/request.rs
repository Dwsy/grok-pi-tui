use super::*;

#[tokio::test]
async fn http_auth_policy_host_and_header_rules() {
    let fixture = Fixture::start(allow(&["http://allowed.example"])).await;
    fixture
        .resolver
        .answer("allowed.example", vec![public_v4(80)]);
    let peer = fixture.connector.push();
    let upstream = http_upstream(
        peer,
        &[
            "proxy-authorization",
            "x-remove",
            "forwarded",
            "x-forwarded-for",
            "via",
        ],
    )
    .await;
    let request = format!(
        "GET http://allowed.example/next?q=1 HTTP/1.1\r\nHost: allowed.example\r\nProxy-Authorization: {}\r\nConnection: X-Remove\r\nX-Remove: yes\r\nForwarded: bad\r\nX-Forwarded-For: bad\r\nVia: bad\r\n\r\n",
        fixture.auth()
    );
    let response = send(fixture.addr(), request.as_bytes()).await;
    assert_eq!(status(&response), 302);
    let response_text = String::from_utf8_lossy(&response);
    assert!(response_text.contains("Location: https://other.example/next"));
    assert!(
        !response_text
            .to_ascii_lowercase()
            .contains("proxy-authentication-info")
    );
    upstream.await.unwrap();
    assert_eq!(fixture.connector.addresses().len(), 1);

    let unauthenticated = send(
        fixture.addr(),
        b"GET http://allowed.example/ HTTP/1.1\r\nHost: allowed.example\r\n\r\n",
    )
    .await;
    assert_eq!(status(&unauthenticated), 407);
    let invalid = format!(
        "GET http://allowed.example/ HTTP/1.1\r\nHost: allowed.example\r\nProxy-Authorization: Bearer wrong\r\n\r\n"
    );
    assert_eq!(status(&send(fixture.addr(), invalid.as_bytes()).await), 407);

    let mismatched = format!(
        "GET http://allowed.example/ HTTP/1.1\r\nHost: other.example\r\nProxy-Authorization: {}\r\n\r\n",
        fixture.auth()
    );
    assert_eq!(
        status(&send(fixture.addr(), mismatched.as_bytes()).await),
        400
    );
    let duplicate = format!(
        "GET http://allowed.example/ HTTP/1.1\r\nHost: allowed.example\r\nHost: allowed.example\r\nProxy-Authorization: {}\r\n\r\n",
        fixture.auth()
    );
    assert_eq!(
        status(&send(fixture.addr(), duplicate.as_bytes()).await),
        400
    );
    fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn request_bounds_reject_oversized_headers_and_bodies() {
    let resolver = Arc::new(MockResolver::default());
    let connector = Arc::new(MockConnector::default());
    let options = EgressProxyOptions {
        max_header_bytes: 256,
        max_headers: 6,
        resolver,
        connector: connector.clone(),
        ..Default::default()
    };
    let handle = EgressProxy::start(allow(&["http://example.com"]), options)
        .await
        .unwrap();
    let large_header = format!(
        "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: {}\r\nX-Large: {}\r\n\r\n",
        handle.proxy_authorization(),
        "x".repeat(256)
    );
    assert_eq!(
        status(&send(handle.address(), large_header.as_bytes()).await),
        400
    );
    let large_body = format!(
        "POST http://example.com/ HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: {}\r\nContent-Length: 3\r\n\r\nabc",
        handle.proxy_authorization()
    );
    assert_eq!(
        status(&send(handle.address(), large_body.as_bytes()).await),
        400
    );
    for headers in [
        "Connection: Content-Length\r\nContent-Length: 0\r\n",
        "Connection: Host\r\n",
        "Connection: Transfer-Encoding\r\nTransfer-Encoding: chunked\r\n",
        "Content-Length: 0\r\nContent-Length: 0\r\n",
        "Expect: 100-continue\r\n",
    ] {
        let request = format!(
            "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: {}\r\n{headers}\r\n",
            handle.proxy_authorization()
        );
        assert_eq!(
            status(&send(handle.address(), request.as_bytes()).await),
            400
        );
    }
    assert!(connector.addresses().is_empty());
    let too_many = format!(
        "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: {}\r\nA: 1\r\nB: 2\r\nC: 3\r\nD: 4\r\nE: 5\r\n\r\n",
        handle.proxy_authorization()
    );
    assert_eq!(
        status(&send(handle.address(), too_many.as_bytes()).await),
        400
    );
    handle.shutdown().await.unwrap();
}
