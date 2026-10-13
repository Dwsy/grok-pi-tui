use base64::Engine;
use tokio::sync::{mpsc, oneshot};
use xai_grok_sandbox::command::{CallId, CommandTag};

use super::*;

#[derive(Clone, Copy)]
pub(super) enum Script {
    AllowAndRecord,
    Deny,
    Ask,
}

/// Replays `script` in order (the last entry repeats); every hold hands its sender to the test.
pub(super) struct ScriptedDecider {
    script: Mutex<VecDeque<Script>>,
    asks: mpsc::UnboundedSender<oneshot::Sender<bool>>,
    pub(super) calls: Mutex<Vec<(String, u16, Option<CommandTag>)>>,
    /// Every connection the proxy asked this decider to park (the card would be raised here).
    pub(super) holds: Mutex<Vec<(String, u16, Option<CommandTag>)>>,
    /// How many times the proxy said it was stopping.
    pub(super) stops: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl Decider for ScriptedDecider {
    async fn decide(&self, origin: &WebsiteOrigin, call: Option<&CommandTag>) -> Decision {
        self.calls.lock().unwrap().push((
            origin.hostname().to_owned(),
            origin.port(),
            call.cloned(),
        ));
        let mut script = self.script.lock().unwrap();
        let next = if script.len() > 1 {
            script.pop_front().unwrap()
        } else {
            *script.front().unwrap()
        };
        match next {
            Script::AllowAndRecord => Decision::AllowAndRecord {
                would: WouldBe::Denied(DenySource::WebFetchDenylist),
            },
            Script::Deny => Decision::Deny,
            Script::Ask => Decision::Ask,
        }
    }

    async fn hold(
        &self,
        origin: &WebsiteOrigin,
        call: Option<&CommandTag>,
    ) -> oneshot::Receiver<bool> {
        self.holds.lock().unwrap().push((
            origin.hostname().to_owned(),
            origin.port(),
            call.cloned(),
        ));
        let (sender, receiver) = oneshot::channel();
        self.asks.send(sender).unwrap();
        receiver
    }

    fn stopping(&self) {
        self.stops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

pub(super) struct AskFixture {
    pub(super) fixture: Fixture,
    pub(super) decider: Arc<ScriptedDecider>,
    asks: mpsc::UnboundedReceiver<oneshot::Sender<bool>>,
    blocked: tokio::sync::broadcast::Receiver<BlockedRequest>,
}

impl AskFixture {
    pub(super) async fn start(
        script: Vec<Script>,
        tune: impl FnOnce(&mut EgressProxyOptions),
    ) -> Self {
        let (asks_tx, asks) = mpsc::unbounded_channel();
        let decider = Arc::new(ScriptedDecider {
            script: Mutex::new(script.into_iter().collect()),
            asks: asks_tx,
            calls: Mutex::new(Vec::new()),
            holds: Mutex::new(Vec::new()),
            stops: std::sync::atomic::AtomicUsize::new(0),
        });
        let resolver = Arc::new(MockResolver::default());
        let connector = Arc::new(MockConnector::default());
        let mut options = EgressProxyOptions {
            resolver: resolver.clone(),
            connector: connector.clone(),
            request_timeout: Duration::from_secs(2),
            dns_timeout: Duration::from_secs(2),
            connect_timeout: Duration::from_secs(2),
            tls_hello_timeout: Duration::from_secs(2),
            drain_timeout: Duration::from_secs(2),
            ..Default::default()
        };
        tune(&mut options);
        let handle = EgressProxy::start_with(
            allow(&["https://held.example", "http://held.example"]),
            decider.clone(),
            options,
        )
        .await
        .unwrap();
        let blocked = handle.blocked_requests();
        AskFixture {
            fixture: Fixture {
                handle,
                resolver,
                connector,
            },
            decider,
            asks,
            blocked,
        }
    }

    pub(super) async fn next_ask(&mut self) -> oneshot::Sender<bool> {
        tokio::time::timeout(Duration::from_secs(2), self.asks.recv())
            .await
            .expect("decider was not asked")
            .unwrap()
    }

    pub(super) async fn next_blocked(&mut self) -> BlockedRequest {
        tokio::time::timeout(Duration::from_secs(2), self.blocked.recv())
            .await
            .expect("no blocked request published")
            .unwrap()
    }
}

/// Opens a CONNECT, asserts nothing came back within a grace period, and returns the stream.
pub(super) async fn open_held_connect(address: SocketAddr, auth: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {auth}\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut probe = [0u8; 1];
    let peeked = tokio::time::timeout(Duration::from_millis(100), stream.read(&mut probe)).await;
    assert!(peeked.is_err(), "held request answered early");
    stream
}

pub(super) async fn finish_connect(mut stream: TcpStream, expected_status: u16) -> Vec<u8> {
    let mut response = Vec::new();
    read_response_head(&mut stream, &mut response).await;
    assert_eq!(expected_status, status(&response));
    if expected_status == 200 {
        stream
            .write_all(&test_client_hello(Some("held.example")))
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
        let _ = stream.read_to_end(&mut response).await;
    }
    response
}

#[tokio::test]
async fn ask_parks_connect_until_allowed_then_relays() {
    let mut ask = AskFixture::start(vec![Script::Ask], |_| {}).await;
    ask.fixture
        .resolver
        .answer("held.example", vec![public_v4(443)]);
    let mut peer = ask.fixture.connector.push();
    let stream = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    let answer = ask.next_ask().await;
    assert!(
        ask.fixture.connector.addresses().is_empty(),
        "connected upstream while held"
    );
    answer.send(true).unwrap();
    let upstream = tokio::spawn(async move {
        let mut received = vec![0; test_client_hello(Some("held.example")).len()];
        peer.read_exact(&mut received).await.unwrap();
        peer.write_all(b"upstream").await.unwrap();
        peer.shutdown().await.unwrap();
    });
    let response = finish_connect(stream, 200).await;
    assert!(response.ends_with(b"upstream"));
    upstream.await.unwrap();
    let blocked = ask.next_blocked().await;
    assert_eq!(("held.example", 443), (blocked.host.as_str(), blocked.port));
    assert_eq!(DeciderOutcome::Allowed, blocked.decided);
    assert_eq!(None, blocked.call);
    assert!(blocked.hold_ms >= 20, "hold_ms={}", blocked.hold_ms);
    assert_eq!(
        vec![("held.example".to_owned(), 443, None)],
        ask.decider.calls.lock().unwrap().clone()
    );
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn ask_denied_answers_403_and_deny_never_holds() {
    let mut ask = AskFixture::start(vec![Script::Ask, Script::Deny], |_| {}).await;
    let stream = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    ask.next_ask().await.send(false).unwrap();
    let response = finish_connect(stream, 403).await;
    assert!(String::from_utf8_lossy(&response).contains("denied"));
    assert_eq!(DeciderOutcome::Denied, ask.next_blocked().await.decided);

    let request = format!(
        "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {}\r\n\r\n",
        ask.fixture.auth()
    );
    let started = std::time::Instant::now();
    assert_eq!(
        403,
        status(&send(ask.fixture.addr(), request.as_bytes()).await)
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    let blocked = ask.next_blocked().await;
    assert_eq!(
        (DeciderOutcome::Denied, 0),
        (blocked.decided, blocked.hold_ms)
    );
    let metrics = ask.fixture.handle.metrics();
    assert_eq!((2, 0), (metrics.policy_denied, metrics.ok));
    ask.fixture.handle.shutdown().await.unwrap();
}

/// A drain with a connection still parked does not wait out its hold: the proxy answers the
/// hold with a deny itself, so `shutdown` returns well inside the drain timeout while the
/// decider still holds the unanswered sender, the client sees 403, the decision is published as
/// denied, and the decider was told exactly once that the proxy is stopping.
#[tokio::test]
async fn a_drain_answers_a_parked_connection_with_a_deny_and_tells_the_decider_once() {
    let mut ask = AskFixture::start(vec![Script::Ask], |options| {
        options.drain_timeout = Duration::from_secs(5);
    })
    .await;
    let stream = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    let unanswered = ask.next_ask().await;
    assert_eq!(1, ask.decider.holds.lock().unwrap().len());
    let mut blocked = ask.fixture.handle.blocked_requests();
    let started = std::time::Instant::now();
    ask.fixture.handle.shutdown().await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the drain waited on the hold for {:?}",
        started.elapsed()
    );
    finish_connect(stream, 403).await;
    let blocked = tokio::time::timeout(Duration::from_secs(2), blocked.recv())
        .await
        .expect("no blocked request published")
        .unwrap();
    assert_eq!(DeciderOutcome::Denied, blocked.decided);
    assert_eq!(
        1,
        ask.decider.stops.load(std::sync::atomic::Ordering::SeqCst)
    );
    drop(unanswered);
}

#[tokio::test]
async fn dropped_ask_sender_is_a_deny() {
    let mut ask = AskFixture::start(vec![Script::Ask], |_| {}).await;
    let stream = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    drop(ask.next_ask().await);
    finish_connect(stream, 403).await;
    assert_eq!(DeciderOutcome::Denied, ask.next_blocked().await.decided);
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn hold_timeout_answers_403_and_publishes_timeout() {
    let mut ask = AskFixture::start(vec![Script::Ask], |options| {
        options.hold_timeout = Duration::from_millis(300);
    })
    .await;
    let stream = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    let _answer = ask.next_ask().await;
    let response = finish_connect(stream, 403).await;
    assert!(String::from_utf8_lossy(&response).contains("denied"));
    let blocked = ask.next_blocked().await;
    assert_eq!(DeciderOutcome::Timeout, blocked.decided);
    assert!(blocked.hold_ms >= 300, "hold_ms={}", blocked.hold_ms);
    let metrics = ask.fixture.handle.metrics();
    assert_eq!(
        (1, 0, 0),
        (metrics.hold_timeout, metrics.timeout, metrics.policy_denied)
    );
    ask.fixture.handle.shutdown().await.unwrap();
}

/// A decider whose `hold` never returns (a card post stuck behind its transport) still meets
/// the hold timeout: the client gets its 403, the hold is published as timed out, and the budget
/// slot comes back for the next connection.
#[tokio::test]
async fn a_decider_stuck_in_hold_still_meets_the_hold_timeout() {
    struct StuckDecider;
    #[async_trait]
    impl Decider for StuckDecider {
        async fn decide(&self, _: &WebsiteOrigin, _: Option<&CommandTag>) -> Decision {
            Decision::Ask
        }
        async fn hold(&self, _: &WebsiteOrigin, _: Option<&CommandTag>) -> oneshot::Receiver<bool> {
            std::future::pending().await
        }
    }
    let resolver = Arc::new(MockResolver::default());
    let connector = Arc::new(MockConnector::default());
    let options = EgressProxyOptions {
        resolver: resolver.clone(),
        connector: connector.clone(),
        hold_timeout: Duration::from_millis(300),
        max_held: 1,
        ..Default::default()
    };
    let handle = EgressProxy::start_with(
        allow(&["https://held.example"]),
        Arc::new(StuckDecider),
        options,
    )
    .await
    .unwrap();
    let mut blocked = handle.blocked_requests();
    let fixture = Fixture {
        handle,
        resolver,
        connector,
    };
    for _ in 0..2 {
        let stream = open_held_connect(fixture.addr(), &fixture.auth()).await;
        tokio::time::timeout(Duration::from_secs(5), finish_connect(stream, 403))
            .await
            .expect("the hold timeout answers");
        let published = tokio::time::timeout(Duration::from_secs(2), blocked.recv())
            .await
            .expect("no blocked request published")
            .unwrap();
        assert_eq!(DeciderOutcome::Timeout, published.decided);
    }
    assert_eq!(0, fixture.handle.metrics().overloaded);
    fixture.handle.shutdown().await.unwrap();
}

/// A decider that never answers `decide` (a stalled read behind it) is cut off at the request
/// timeout: the client gets its `504` instead of the connection staying pinned.
#[tokio::test]
async fn a_decider_stuck_in_decide_is_cut_off_at_the_request_timeout() {
    struct StuckDecide;
    #[async_trait]
    impl Decider for StuckDecide {
        async fn decide(&self, _: &WebsiteOrigin, _: Option<&CommandTag>) -> Decision {
            std::future::pending().await
        }
    }
    let resolver = Arc::new(MockResolver::default());
    let connector = Arc::new(MockConnector::default());
    let handle = EgressProxy::start_with(
        allow(&["https://held.example"]),
        Arc::new(StuckDecide),
        EgressProxyOptions {
            resolver: resolver.clone(),
            connector: connector.clone(),
            request_timeout: Duration::from_millis(300),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let fixture = Fixture {
        handle,
        resolver,
        connector,
    };
    let request = format!(
        "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {}\r\n\r\n",
        fixture.auth()
    );
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        send(fixture.addr(), request.as_bytes()),
    )
    .await
    .expect("the request timeout answers");
    assert_eq!(504, status(&response));
    fixture.handle.shutdown().await.unwrap();
}

/// A client that sends more than the held buffer allows is refused as malformed while still
/// connected: the hold is published as denied, not as a hang-up a subscriber would wait out.
#[tokio::test]
async fn an_oversized_held_client_is_published_as_denied() {
    let mut ask = AskFixture::start(vec![Script::Ask], |_| {}).await;
    let mut stream = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    let _answer = ask.next_ask().await;
    let _ = stream.write_all(&vec![0u8; 80 * 1024]).await;
    assert_eq!(DeciderOutcome::Denied, ask.next_blocked().await.decided);
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn hold_outlives_request_timeout() {
    let mut ask = AskFixture::start(vec![Script::Ask], |options| {
        options.request_timeout = Duration::from_millis(150);
        options.tls_hello_timeout = Duration::from_millis(150);
    })
    .await;
    ask.fixture
        .resolver
        .answer("held.example", vec![public_v4(443)]);
    let mut peer = ask.fixture.connector.push();
    let stream = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    let answer = ask.next_ask().await;
    // The scenario itself: stay held past `request_timeout` and `tls_hello_timeout` (150 ms) so
    // the answer arrives only after both would have fired. Bounded by the constants above.
    tokio::time::sleep(Duration::from_millis(400)).await;
    answer.send(true).unwrap();
    let upstream = tokio::spawn(async move {
        let mut received = vec![0; test_client_hello(Some("held.example")).len()];
        peer.read_exact(&mut received).await.unwrap();
        peer.shutdown().await.unwrap();
    });
    finish_connect(stream, 200).await;
    upstream.await.unwrap();
    assert_eq!(DeciderOutcome::Allowed, ask.next_blocked().await.decided);
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn max_held_overflow_answers_503_and_one_answer_releases_every_held_request() {
    let mut ask = AskFixture::start(vec![Script::Ask], |options| {
        options.max_held = 2;
    })
    .await;
    let first = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    let second = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    let answers = [ask.next_ask().await, ask.next_ask().await];
    let request = format!(
        "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {}\r\n\r\n",
        ask.fixture.auth()
    );
    let overflow = send(ask.fixture.addr(), request.as_bytes()).await;
    assert_eq!(503, status(&overflow));
    assert!(String::from_utf8_lossy(&overflow).contains("overloaded"));
    // The overflow was decided but never parked: no card for a connection the client saw refused
    assert_eq!(3, ask.decider.calls.lock().unwrap().len());
    assert_eq!(2, ask.decider.holds.lock().unwrap().len());
    assert_eq!(1, ask.fixture.handle.metrics().overloaded);

    for answer in answers {
        answer.send(false).unwrap();
    }
    finish_connect(first, 403).await;
    finish_connect(second, 403).await;
    assert_eq!(DeciderOutcome::Denied, ask.next_blocked().await.decided);
    assert_eq!(DeciderOutcome::Denied, ask.next_blocked().await.decided);
    // Both permits are back: a third ask is held, not refused.
    let third = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    ask.next_ask().await.send(false).unwrap();
    finish_connect(third, 403).await;
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn client_hangup_while_held_frees_the_slot_and_publishes_abandoned() {
    let mut ask = AskFixture::start(vec![Script::Ask], |options| {
        options.max_held = 1;
    })
    .await;
    let stream = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    let answer = ask.next_ask().await;
    drop(stream);
    let blocked = ask.next_blocked().await;
    assert_eq!(DeciderOutcome::Abandoned, blocked.decided);
    assert!(answer.is_closed());
    assert_eq!(1, ask.fixture.handle.metrics().abandoned);
    let next = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    ask.next_ask().await.send(false).unwrap();
    finish_connect(next, 403).await;
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn plain_http_ask_holds_then_forwards() {
    let mut ask = AskFixture::start(vec![Script::Ask], |_| {}).await;
    ask.fixture
        .resolver
        .answer("held.example", vec![public_v4(80)]);
    let peer = ask.fixture.connector.push();
    let upstream = http_upstream(peer, &[]).await;
    let mut stream = TcpStream::connect(ask.fixture.addr()).await.unwrap();
    let request = format!(
        "GET http://held.example/ HTTP/1.1\r\nHost: held.example\r\nProxy-Authorization: {}\r\n\r\n",
        ask.fixture.auth()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let answer = ask.next_ask().await;
    let mut probe = [0u8; 1];
    assert!(
        tokio::time::timeout(Duration::from_millis(100), stream.read(&mut probe))
            .await
            .is_err()
    );
    answer.send(true).unwrap();
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response).await;
    assert_eq!(302, status(&response));
    upstream.await.unwrap();
    assert_eq!(DeciderOutcome::Allowed, ask.next_blocked().await.decided);
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn allow_and_record_allows_and_publishes_observed_without_holding() {
    let mut ask = AskFixture::start(vec![Script::AllowAndRecord], |_| {}).await;
    ask.fixture
        .resolver
        .answer("held.example", vec![public_v4(80)]);
    let upstream = http_upstream(ask.fixture.connector.push(), &[]).await;
    let request = format!(
        "GET http://held.example/ HTTP/1.1\r\nHost: held.example\r\nProxy-Authorization: {}\r\n\r\n",
        ask.fixture.auth()
    );
    assert_eq!(
        302,
        status(&send(ask.fixture.addr(), request.as_bytes()).await)
    );
    upstream.await.unwrap();
    let blocked = ask.next_blocked().await;
    // The would-be verdict rides the record as the decider gave it
    assert_eq!(
        (
            DeciderOutcome::Observed {
                would: WouldBe::Denied(DenySource::WebFetchDenylist)
            },
            0
        ),
        (blocked.decided, blocked.hold_ms)
    );
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn policy_denial_is_answered_before_the_decider_runs() {
    let ask = AskFixture::start(vec![Script::Ask], |_| {}).await;
    let request = format!(
        "CONNECT other.example:443 HTTP/1.1\r\nHost: other.example:443\r\nProxy-Authorization: {}\r\n\r\n",
        ask.fixture.auth()
    );
    assert_eq!(
        403,
        status(&send(ask.fixture.addr(), request.as_bytes()).await)
    );
    assert!(ask.decider.calls.lock().unwrap().is_empty());
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn zero_max_held_is_an_invalid_option() {
    let options = EgressProxyOptions {
        max_held: 0,
        ..Default::default()
    };
    let error = EgressProxy::start_with(allow(&[]), Arc::new(PolicyOnly), options)
        .await
        .err()
        .unwrap();
    assert!(matches!(error, ProxyError::InvalidOptions));
}

#[tokio::test]
async fn call_credential_attributes_the_decision_and_revocation_answers_407() {
    let mut ask = AskFixture::start(vec![Script::Deny], |_| {}).await;
    let tag = CommandTag::for_call(&CallId::tool("call-1"));
    let credential = ask.fixture.handle.mint_call_credential(&tag).unwrap();
    let url = credential.proxy_url(ask.fixture.addr());
    assert!(url.starts_with(&format!("http://{PROXY_USERNAME}:")));
    assert!(url.ends_with(&ask.fixture.addr().to_string()));
    let request = format!(
        "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {}\r\n\r\n",
        credential.proxy_authorization()
    );
    assert_eq!(
        403,
        status(&send(ask.fixture.addr(), request.as_bytes()).await)
    );
    assert_eq!(Some(tag.clone()), ask.next_blocked().await.call);
    assert_eq!(
        vec![("held.example".to_owned(), 443, Some(tag.clone()))],
        ask.decider.calls.lock().unwrap().clone()
    );

    assert!(ask.fixture.handle.revoke_call_credential(&tag));
    assert!(!ask.fixture.handle.revoke_call_credential(&tag));
    let response = send(ask.fixture.addr(), request.as_bytes()).await;
    assert_eq!(407, status(&response));
    assert!(String::from_utf8_lossy(&response).contains("Proxy-Authenticate: Basic"));
    assert_eq!(1, ask.decider.calls.lock().unwrap().len());
    assert_eq!(1, ask.fixture.handle.metrics().unauthenticated);
    ask.fixture.handle.shutdown().await.unwrap();
}

/// A decider that admits unauthenticated requests (the sandbox under `observe`) decides a
/// request with a revoked or missing credential as the session's instead of the proxy
/// answering `407`.
#[tokio::test]
async fn a_decider_admitting_unauthenticated_requests_decides_them_as_the_sessions() {
    struct Admitting(Mutex<Vec<Option<CommandTag>>>);
    #[async_trait]
    impl Decider for Admitting {
        async fn decide(&self, _: &WebsiteOrigin, call: Option<&CommandTag>) -> Decision {
            self.0.lock().unwrap().push(call.cloned());
            Decision::Deny
        }
        fn admits_unauthenticated(&self) -> bool {
            true
        }
    }
    let decider = Arc::new(Admitting(Mutex::new(Vec::new())));
    let resolver = Arc::new(MockResolver::default());
    let connector = Arc::new(MockConnector::default());
    let handle = EgressProxy::start_with(
        allow(&["https://held.example"]),
        decider.clone(),
        EgressProxyOptions {
            resolver: resolver.clone(),
            connector: connector.clone(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let fixture = Fixture {
        handle,
        resolver,
        connector,
    };
    let tag = CommandTag::for_call(&CallId::tool("call-ended"));
    let credential = fixture.handle.mint_call_credential(&tag).unwrap();
    assert!(fixture.handle.revoke_call_credential(&tag));
    for authorization in [
        format!(
            "Proxy-Authorization: {}\r\n",
            credential.proxy_authorization()
        ),
        String::new(),
    ] {
        let request = format!(
            "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\n{authorization}\r\n"
        );
        assert_eq!(403, status(&send(fixture.addr(), request.as_bytes()).await));
    }
    assert_eq!(vec![None, None], decider.0.lock().unwrap().clone());
    fixture.handle.shutdown().await.unwrap();
}

/// The secret leaves a credential only through the URL and header builders; `{:?}` of
/// the credential (the shape a log line or an `assert_eq!` failure prints) redacts it.
#[tokio::test]
async fn a_credentials_debug_output_never_carries_its_token() {
    let ask = AskFixture::start(vec![Script::Deny], |_| {}).await;
    let tag = CommandTag::for_call(&CallId::tool("call-debug"));
    let credential = ask.fixture.handle.mint_call_credential(&tag).unwrap();
    let url = credential.proxy_url(ask.fixture.addr());
    let token = url
        .strip_prefix("http://grok:")
        .and_then(|rest| rest.split_once('@'))
        .map(|(token, _)| token.to_owned())
        .unwrap();
    assert!(token.len() >= 32, "{url}");
    let debug = format!("{credential:?}");
    assert!(debug.contains("ProxyToken(***)"), "{debug}");
    assert!(!debug.contains(&token), "{debug}");
    let session = ask.fixture.auth();
    let session_token = session.strip_prefix("Bearer ").unwrap();
    assert!(!debug.contains(session_token), "{debug}");
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn minting_again_replaces_the_previous_call_credential() {
    let ask = AskFixture::start(vec![Script::Deny], |_| {}).await;
    let tag = CommandTag::for_call(&CallId::tool("call-2"));
    let stale = ask.fixture.handle.mint_call_credential(&tag).unwrap();
    let fresh = ask.fixture.handle.mint_call_credential(&tag).unwrap();
    assert_ne!(stale, fresh);
    for (credential, expected) in [(&stale, 407), (&fresh, 403)] {
        let request = format!(
            "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {}\r\n\r\n",
            credential.proxy_authorization()
        );
        assert_eq!(
            expected,
            status(&send(ask.fixture.addr(), request.as_bytes()).await)
        );
    }
    ask.fixture.handle.shutdown().await.unwrap();
}

/// A guard that minted the earlier credential and revokes it late must not take the newer one
/// with it: revoking by credential leaves a replacement for the same tag alone.
#[tokio::test]
async fn revoking_a_replaced_credential_leaves_the_tags_live_one() {
    let ask = AskFixture::start(vec![Script::Deny], |_| {}).await;
    let tag = CommandTag::for_call(&CallId::tool("call-3"));
    let stale = ask.fixture.handle.mint_call_credential(&tag).unwrap();
    let fresh = ask.fixture.handle.mint_call_credential(&tag).unwrap();
    assert!(
        !ask.fixture.handle.revoke_credential(&stale),
        "the mint replaced it already"
    );
    let request = format!(
        "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {}\r\n\r\n",
        fresh.proxy_authorization()
    );
    assert_eq!(
        403,
        status(&send(ask.fixture.addr(), request.as_bytes()).await)
    );
    assert!(ask.fixture.handle.revoke_credential(&fresh));
    assert_eq!(
        407,
        status(&send(ask.fixture.addr(), request.as_bytes()).await)
    );
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn session_token_is_accepted_as_bearer_or_basic_password() {
    let ask = AskFixture::start(vec![Script::Deny], |_| {}).await;
    let bearer = ask.fixture.auth();
    let token = bearer.strip_prefix("Bearer ").unwrap();
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("anyone:{token}"))
    );
    let wrong = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{token}:"))
    );
    for (auth, expected) in [
        (bearer.as_str(), 403),
        (basic.as_str(), 403),
        (wrong.as_str(), 407),
        ("Basic not-base64!", 407),
        ("Digest abc", 407),
    ] {
        let request = format!(
            "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {auth}\r\n\r\n"
        );
        assert_eq!(
            expected,
            status(&send(ask.fixture.addr(), request.as_bytes()).await),
            "{auth}"
        );
    }
    ask.fixture.handle.shutdown().await.unwrap();
}
