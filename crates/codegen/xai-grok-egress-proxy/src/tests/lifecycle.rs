use super::*;

#[tokio::test]
async fn completed_connection_tasks_are_reaped() {
    let fixture = Fixture::start(allow(&[])).await;
    for _ in 0..200 {
        let response = send(
            fixture.addr(),
            b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
        )
        .await;
        assert_eq!(status(&response), 407);
    }
    for _ in 0..20 {
        if fixture.handle.tracked_tasks() <= 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(fixture.handle.tracked_tasks() <= 1);
    fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn connection_limit_returns_overload() {
    let resolver = Arc::new(MockResolver::default());
    let connector = Arc::new(MockConnector::default());
    let options = EgressProxyOptions {
        max_connections: 1,
        resolver,
        connector,
        ..Default::default()
    };
    let handle = EgressProxy::start(allow(&[]), options).await.unwrap();
    let held = TcpStream::connect(handle.address()).await.unwrap();
    wait_for_tracked_tasks(&handle, 1).await;
    let response = send(
        handle.address(),
        b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
    )
    .await;
    assert_eq!(status(&response), 503);
    drop(held);
    wait_for_tracked_tasks(&handle, 0).await;
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_waits_for_active_tunnel_within_bound() {
    let fixture = Fixture::start(allow(&["https://allowed.example"])).await;
    fixture
        .resolver
        .answer("allowed.example", vec![public_v4(443)]);
    let mut peer = fixture.connector.push();
    let address = fixture.addr();
    let auth = fixture.auth();
    let client = tokio::spawn(async move {
        let mut stream = TcpStream::connect(address).await.unwrap();
        let request = format!(
            "CONNECT allowed.example:443 HTTP/1.1\r\nHost: allowed.example:443\r\nProxy-Authorization: {auth}\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        read_response_head(&mut stream, &mut response).await;
        stream
            .write_all(&test_client_hello(Some("allowed.example")))
            .await
            .unwrap();
        let _ = stream.read_to_end(&mut response).await;
    });
    let mut received = vec![0; test_client_hello(Some("allowed.example")).len()];
    peer.read_exact(&mut received).await.unwrap();
    drop(peer);
    fixture.handle.shutdown().await.unwrap();
    client.await.unwrap();
}

#[tokio::test]
async fn explicit_abort_joins_active_tasks_and_drops_resources() {
    let resolver = Arc::new(MockResolver::default());
    resolver.answer("allowed.example", vec![public_v4(80)]);
    let connector = Arc::new(MockConnector::default());
    let (dropped_tx, mut dropped_rx) = tokio::sync::oneshot::channel();
    connector.push_io(HeldIo {
        _drop_signal: DropSignal(Some(dropped_tx)),
    });
    let mut fixture =
        Fixture::start_with(allow(&["http://allowed.example"]), resolver, connector).await;
    let address = fixture.addr();
    let auth = fixture.auth();
    let client = tokio::spawn(async move {
        let mut stream = TcpStream::connect(address).await.unwrap();
        let request = format!(
            "GET http://allowed.example/ HTTP/1.1\r\nHost: allowed.example\r\nProxy-Authorization: {auth}\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response).await;
    });
    for _ in 0..20 {
        if fixture.handle.tracked_tasks() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.handle.tracked_tasks(), 1);
    assert!(dropped_rx.try_recv().is_err());
    fixture.handle.abort().await.unwrap();
    dropped_rx.await.unwrap();
    assert_eq!(fixture.handle.tracked_tasks(), 0);
    assert!(!fixture.handle.is_running());
    assert!(TcpStream::connect(address).await.is_err());
    client.await.unwrap();
}

#[tokio::test]
async fn lifecycle_is_ready_and_shutdown_leaves_no_fallback_listener() {
    let fixture = Fixture::start(allow(&[])).await;
    assert!(fixture.handle.is_running());
    let address = fixture.addr();
    assert!(TcpStream::connect(address).await.is_ok());
    fixture.handle.shutdown().await.unwrap();
    assert!(TcpStream::connect(address).await.is_err());
}
