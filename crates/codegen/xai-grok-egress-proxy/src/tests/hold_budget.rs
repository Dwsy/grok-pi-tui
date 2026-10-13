//! The two halves of the hold budget: `max_held` across the proxy and
//! `max_held_per_call` per credential, plus the `deadline_unix` every held decision carries.

use xai_grok_sandbox::command::grants::FixedClock;
use xai_grok_sandbox::command::{CallId, CommandTag};

use super::decider::{AskFixture, Script, finish_connect, open_held_connect};
use super::*;
use crate::decider::HoldBudget;

#[test]
fn defaults_match_the_hub_backstop_and_nest_the_per_call_cap() {
    assert_eq!(Duration::from_secs(600), DEFAULT_HOLD_TIMEOUT);
    assert_eq!((16, 8), (DEFAULT_MAX_HELD, DEFAULT_MAX_HELD_PER_CALL));
    let options = EgressProxyOptions::default();
    assert_eq!(
        (DEFAULT_MAX_HELD, DEFAULT_MAX_HELD_PER_CALL),
        (options.max_held, options.max_held_per_call)
    );
}

#[test]
fn hold_budget_permits_hand_the_share_back_on_drop() {
    let budget = HoldBudget::new(2);
    let call = CommandTag::for_call(&CallId::tool("budget-1"));
    let other = CommandTag::for_call(&CallId::tool("budget-2"));
    let first = budget.try_take(Some(&call)).unwrap();
    let second = budget.try_take(Some(&call)).unwrap();
    assert!(budget.try_take(Some(&call)).is_none());
    assert_eq!(2, budget.held_by(Some(&call)));
    // Another call and the session credential have their own shares.
    let _theirs = budget.try_take(Some(&other)).unwrap();
    let _session = budget.try_take(None).unwrap();
    drop(first);
    assert_eq!(1, budget.held_by(Some(&call)));
    let _third = budget.try_take(Some(&call)).unwrap();
    drop(second);
    drop(_third);
    assert_eq!(0, budget.held_by(Some(&call)));
    assert_eq!((1, 1), (budget.held_by(Some(&other)), budget.held_by(None)));
}

#[tokio::test]
async fn zero_per_call_cap_is_an_invalid_option() {
    let options = EgressProxyOptions {
        max_held_per_call: 0,
        ..Default::default()
    };
    let error = EgressProxy::start_with(allow(&[]), Arc::new(PolicyOnly), options)
        .await
        .err()
        .unwrap();
    assert!(matches!(error, ProxyError::InvalidOptions));
}

/// Lowering `max_held` alone (the default per-call cap stays 8) must not reject the options or
/// leave a per-call share the proxy-wide budget cannot honour.
#[tokio::test]
async fn per_call_cap_is_clamped_to_max_held() {
    let mut ask = AskFixture::start(vec![Script::Ask], |options| {
        options.max_held = 1;
    })
    .await;
    assert_eq!(1, ask.fixture.handle.max_held_per_call());
    let held = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    let answer = ask.next_ask().await;
    let request = format!(
        "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {}\r\n\r\n",
        ask.fixture.auth()
    );
    assert_eq!(
        503,
        status(&send(ask.fixture.addr(), request.as_bytes()).await)
    );
    assert_eq!(
        1,
        ask.decider.holds.lock().unwrap().len(),
        "the refused connection was never parked"
    );
    answer.send(false).unwrap();
    finish_connect(held, 403).await;
    ask.fixture.handle.shutdown().await.unwrap();
}

/// The hold budget is per call: a command whose asks go unanswered is told 503 at its own cap
/// while a second command's asks still hold, instead of pinning every slot.
#[tokio::test]
async fn one_call_at_its_cap_is_refused_while_another_call_still_holds() {
    let mut ask = AskFixture::start(vec![Script::Ask], |options| {
        options.max_held = 4;
        options.max_held_per_call = 2;
    })
    .await;
    let greedy = CommandTag::for_call(&CallId::tool("greedy"));
    let polite = CommandTag::for_call(&CallId::tool("polite"));
    let greedy_auth = ask
        .fixture
        .handle
        .mint_call_credential(&greedy)
        .unwrap()
        .proxy_authorization();
    let polite_auth = ask
        .fixture
        .handle
        .mint_call_credential(&polite)
        .unwrap()
        .proxy_authorization();

    let greedy_held = [
        open_held_connect(ask.fixture.addr(), &greedy_auth).await,
        open_held_connect(ask.fixture.addr(), &greedy_auth).await,
    ];
    let mut answers = vec![ask.next_ask().await, ask.next_ask().await];
    let request = format!(
        "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {greedy_auth}\r\n\r\n"
    );
    let refused = send(ask.fixture.addr(), request.as_bytes()).await;
    assert_eq!(503, status(&refused));
    assert!(String::from_utf8_lossy(&refused).contains("overloaded"));
    // The refused connection raised no card
    assert_eq!(2, ask.decider.holds.lock().unwrap().len());
    assert_eq!(1, ask.fixture.handle.metrics().overloaded);

    // Two proxy-wide permits remain and the polite command owns its own share of them.
    let polite_held = [
        open_held_connect(ask.fixture.addr(), &polite_auth).await,
        open_held_connect(ask.fixture.addr(), &polite_auth).await,
    ];
    answers.push(ask.next_ask().await);
    answers.push(ask.next_ask().await);
    // Now the proxy is full: the polite command is refused by the global budget, not its own.
    let request = format!(
        "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {polite_auth}\r\n\r\n"
    );
    assert_eq!(
        503,
        status(&send(ask.fixture.addr(), request.as_bytes()).await)
    );
    assert_eq!(4, ask.decider.holds.lock().unwrap().len());
    assert_eq!(2, ask.fixture.handle.metrics().overloaded);

    for answer in answers {
        answer.send(false).unwrap();
    }
    for stream in greedy_held.into_iter().chain(polite_held) {
        finish_connect(stream, 403).await;
    }
    for _ in 0..4 {
        let blocked = ask.next_blocked().await;
        assert_eq!(DeciderOutcome::Denied, blocked.decided);
        assert!(blocked.deadline_unix.is_some());
    }
    // Every share is back: the greedy command may hold again.
    let again = open_held_connect(ask.fixture.addr(), &greedy_auth).await;
    ask.next_ask().await.send(false).unwrap();
    finish_connect(again, 403).await;
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn session_credential_has_its_own_share_of_the_budget() {
    let mut ask = AskFixture::start(vec![Script::Ask], |options| {
        options.max_held = 3;
        options.max_held_per_call = 1;
    })
    .await;
    let session = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    let answer = ask.next_ask().await;
    let request = format!(
        "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {}\r\n\r\n",
        ask.fixture.auth()
    );
    assert_eq!(
        503,
        status(&send(ask.fixture.addr(), request.as_bytes()).await)
    );
    assert_eq!(1, ask.decider.holds.lock().unwrap().len());
    let call_auth = ask
        .fixture
        .handle
        .mint_call_credential(&CommandTag::for_call(&CallId::tool("call-3")))
        .unwrap()
        .proxy_authorization();
    let call = open_held_connect(ask.fixture.addr(), &call_auth).await;
    let call_answer = ask.next_ask().await;
    answer.send(false).unwrap();
    call_answer.send(false).unwrap();
    finish_connect(session, 403).await;
    finish_connect(call, 403).await;
    ask.fixture.handle.shutdown().await.unwrap();
}

/// The record says when the proxy itself would answer 403, in the same unix
/// seconds the card counts down from, so the card and the held connection end together.
#[tokio::test]
async fn held_decisions_carry_the_deadline_from_the_injected_clock() {
    let clock = Arc::new(FixedClock::at(1_700_000_000));
    let mut ask = AskFixture::start(vec![Script::Ask], |options| {
        options.clock = clock.clone();
        options.hold_timeout = Duration::from_secs(90);
    })
    .await;
    let stream = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    ask.next_ask().await.send(false).unwrap();
    finish_connect(stream, 403).await;
    let blocked = ask.next_blocked().await;
    assert_eq!(
        (DeciderOutcome::Denied, Some(1_700_000_090)),
        (blocked.decided, blocked.deadline_unix)
    );

    clock.advance(1_000);
    let stream = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    ask.next_ask().await.send(false).unwrap();
    finish_connect(stream, 403).await;
    assert_eq!(Some(1_700_001_090), ask.next_blocked().await.deadline_unix);
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn timed_out_hold_reports_the_deadline_it_enforced() {
    let mut ask = AskFixture::start(vec![Script::Ask], |options| {
        options.clock = Arc::new(FixedClock::at(2_000));
        options.hold_timeout = Duration::from_secs(1);
    })
    .await;
    let stream = open_held_connect(ask.fixture.addr(), &ask.fixture.auth()).await;
    let _unanswered = ask.next_ask().await;
    finish_connect(stream, 403).await;
    let blocked = ask.next_blocked().await;
    assert_eq!(
        (DeciderOutcome::Timeout, Some(2_001)),
        (blocked.decided, blocked.deadline_unix)
    );
    ask.fixture.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn decisions_that_never_held_carry_no_deadline() {
    let mut ask = AskFixture::start(vec![Script::Deny], |_| {}).await;
    let request = format!(
        "CONNECT held.example:443 HTTP/1.1\r\nHost: held.example:443\r\nProxy-Authorization: {}\r\n\r\n",
        ask.fixture.auth()
    );
    assert_eq!(
        403,
        status(&send(ask.fixture.addr(), request.as_bytes()).await)
    );
    let blocked = ask.next_blocked().await;
    assert_eq!(
        (DeciderOutcome::Denied, None),
        (blocked.decided, blocked.deadline_unix)
    );
    ask.fixture.handle.shutdown().await.unwrap();
}
