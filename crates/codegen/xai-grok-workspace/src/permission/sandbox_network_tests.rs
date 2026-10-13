use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::oneshot;
use xai_grok_egress_proxy::{DEFAULT_HOLD_TIMEOUT, Decider, Decision, DenySource, WouldBe};
use xai_grok_sandbox::WebsiteOrigin;
use xai_grok_sandbox::command::{
    Blocked, CallId, CommandTag, Expiry, FixedClock, Grant, GrantDecision, GrantId, GrantScope,
    GrantSubject, HostPattern, InformationalReason, Replay, SandboxMode, Violation,
};

use super::{
    GrantView, HoldAnswer, MAX_PENDING_CARDS, SandboxNetworkDecider, SandboxNetworkDeciderConfig,
    ViolationSink, WebFetchDomains,
};

/// What the view runs, once, while a reader awaits its lists: what lands during that await.
type DuringListsRead = Box<dyn FnOnce() + Send>;

/// One call the stub's table holds: the mode its spawn record pins (`None` for an entry with
/// neither a spawn nor a pin), its epoch, and whether its spawn record is still open.
struct StubCall {
    tag: CommandTag,
    mode: Option<SandboxMode>,
    epoch: u64,
    runs: bool,
}

/// Stands in for the sandbox: the mode, rows and domain sets the test sets are what the decider
/// reads; `call_rows` are the one-shot rows stashed for a running call, `calls` the table's
/// entries.
struct StubView {
    mode: Arc<Mutex<SandboxMode>>,
    /// Runs during the `n`th lists read (0-based), then is gone.
    during_lists_read: Mutex<Option<(usize, DuringListsRead)>>,
    lists_reads: AtomicUsize,
    rows: Mutex<Vec<Grant>>,
    call_rows: Mutex<Vec<(CommandTag, Grant)>>,
    calls: Mutex<Vec<StubCall>>,
    domains: Mutex<WebFetchDomains>,
    reads: Mutex<Vec<Option<CommandTag>>>,
}

impl StubView {
    fn with_mode(mode: SandboxMode) -> StubView {
        StubView {
            mode: Arc::new(Mutex::new(mode)),
            during_lists_read: Mutex::default(),
            lists_reads: AtomicUsize::new(0),
            rows: Mutex::default(),
            call_rows: Mutex::default(),
            calls: Mutex::default(),
            domains: Mutex::default(),
            reads: Mutex::default(),
        }
    }
}

#[async_trait]
impl GrantView for StubView {
    fn mode(&self) -> SandboxMode {
        *self.mode.lock()
    }

    fn net_rows(&self, call: Option<&CommandTag>) -> Vec<Grant> {
        self.reads.lock().push(call.cloned());
        let mut rows = self.rows.lock().clone();
        rows.extend(
            self.call_rows
                .lock()
                .iter()
                .filter(|(owner, _)| Some(owner) == call)
                .map(|(_, grant)| grant.clone()),
        );
        rows
    }

    fn call_mode(&self, call: Option<&CommandTag>) -> Option<SandboxMode> {
        let calls = self.calls.lock();
        call.and_then(|call| calls.iter().find(|entry| entry.tag == *call))
            .and_then(|entry| entry.mode)
    }

    fn call_epoch(&self, call: Option<&CommandTag>) -> Option<u64> {
        let calls = self.calls.lock();
        call.and_then(|call| calls.iter().find(|entry| entry.tag == *call))
            .map(|entry| entry.epoch)
    }

    fn call_runs(&self, call: &CommandTag) -> bool {
        self.calls
            .lock()
            .iter()
            .any(|entry| entry.tag == *call && entry.runs)
    }

    fn any_call_runs_enforced(&self) -> bool {
        self.calls
            .lock()
            .iter()
            .any(|entry| entry.mode == Some(SandboxMode::Enforce))
    }

    async fn web_fetch_domains(&self) -> WebFetchDomains {
        let nth = self.lists_reads.fetch_add(1, Ordering::SeqCst);
        let due = {
            let mut hook = self.during_lists_read.lock();
            match hook.take() {
                Some((at, then)) if at == nth => Some(then),
                other => {
                    *hook = other;
                    None
                }
            }
        };
        if let Some(then) = due {
            then();
            tokio::task::yield_now().await;
        }
        self.domains.lock().clone()
    }
}

impl StubView {
    fn set_rows(&self, rows: impl IntoIterator<Item = Grant>) {
        *self.rows.lock() = rows.into_iter().collect();
    }

    /// The table holds `call` with `mode` pinned (its spawn record's, or the hub's pin), at
    /// epoch 1.
    fn pin(&self, call: &CommandTag, mode: SandboxMode) {
        self.enter(call, Some(mode));
    }

    /// The table holds `call` with neither a spawn nor a pin: a known tag with no mode.
    fn enter(&self, call: &CommandTag, mode: Option<SandboxMode>) {
        let mut calls = self.calls.lock();
        calls.retain(|entry| entry.tag != *call);
        calls.push(StubCall {
            tag: call.clone(),
            mode,
            epoch: 1,
            runs: true,
        });
    }

    /// `call` went to the background, or a later result moved its epoch on.
    fn bump_epoch(&self, call: &CommandTag) -> u64 {
        let mut calls = self.calls.lock();
        let entry = calls
            .iter_mut()
            .find(|entry| entry.tag == *call)
            .expect("the table holds the call");
        entry.epoch += 1;
        entry.epoch
    }

    /// `call`'s background child exited with the hub still owning it: its spawn record is gone
    /// and its epoch moved on, the entry kept for the result.
    fn end_spawn(&self, call: &CommandTag) {
        let mut calls = self.calls.lock();
        let entry = calls
            .iter_mut()
            .find(|entry| entry.tag == *call)
            .expect("the table holds the call");
        entry.runs = false;
        entry.epoch += 1;
    }

    /// `call` ended: the table let its entry go.
    fn unpin(&self, call: &CommandTag) {
        self.calls.lock().retain(|entry| entry.tag != *call);
    }

    /// The folder's mode flips to `to` during the `nth` lists read (0-based) from now.
    fn flip_during_lists_read(&self, nth: usize, to: SandboxMode) {
        let mode = self.mode.clone();
        self.run_during_lists_read(nth, move || *mode.lock() = to);
    }

    /// `then` runs during the `nth` lists read (0-based) from now: what lands mid-decision, in
    /// the one await a decision or a park has before its lock.
    fn run_during_lists_read(&self, nth: usize, then: impl FnOnce() + Send + 'static) {
        self.lists_reads.store(0, Ordering::SeqCst);
        *self.during_lists_read.lock() = Some((nth, Box::new(then)));
    }

    fn set_domains(&self, allowed: &[&str], disallowed: &[&str]) {
        *self.domains.lock() = WebFetchDomains {
            allowed: allowed.iter().map(|d| (*d).to_owned()).collect(),
            disallowed: disallowed.iter().map(|d| (*d).to_owned()).collect(),
        };
    }
}

#[derive(Default)]
struct RecordingSink {
    posted: Mutex<Vec<(Option<CommandTag>, Violation)>>,
    /// While set, `post` never returns: a gate too slow to enqueue, so a test can drop the
    /// park while it waits there.
    stalled: AtomicBool,
    /// Runs once, while the next `post` is in progress: what lands during the card's post.
    during_post: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

#[async_trait]
impl ViolationSink for RecordingSink {
    async fn post(&self, call: Option<&CommandTag>, violation: Violation) {
        if self.stalled.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        let during_post = self.during_post.lock().take();
        if let Some(then) = during_post {
            then();
            tokio::task::yield_now().await;
        }
        self.posted.lock().push((call.cloned(), violation));
    }
}

impl RecordingSink {
    /// The ids of the grantable cards posted (the holds), in order.
    fn hold_ids(&self) -> Vec<String> {
        self.posted
            .lock()
            .iter()
            .filter(|(_, violation)| violation.is_grantable())
            .map(|(_, violation)| match &violation.replay {
                Replay::Resume { hold_id } => hold_id.clone(),
                Replay::Rerun => panic!("network violations resume"),
            })
            .collect()
    }

    /// The informational cards posted, as `(call, host:port, reason)`.
    fn informational(&self) -> Vec<(Option<CommandTag>, String, InformationalReason)> {
        self.posted
            .lock()
            .iter()
            .filter(|(_, violation)| !violation.is_grantable())
            .map(|(call, violation)| {
                let target = match &violation.blocked {
                    Blocked::Net {
                        host: Some(host),
                        port: Some(port),
                    } => format!("{host}:{port}"),
                    other => panic!("informational network card without an origin: {other:?}"),
                };
                let reason = violation
                    .disposition
                    .reason()
                    .expect("an informational card carries its reason");
                (call.clone(), target, reason)
            })
            .collect()
    }
}

struct Harness {
    decider: Arc<SandboxNetworkDecider>,
    sink: Arc<RecordingSink>,
    clock: Arc<FixedClock>,
    view: Arc<StubView>,
    mode: Arc<Mutex<SandboxMode>>,
}

const NOW: i64 = 1_700_000_000;

async fn harness(mode: SandboxMode, with_sink: bool) -> Harness {
    let sink = Arc::new(RecordingSink::default());
    let clock = Arc::new(FixedClock::at(NOW));
    let view = Arc::new(StubView::with_mode(mode));
    let decider = Arc::new(SandboxNetworkDecider::new(SandboxNetworkDeciderConfig {
        clock: clock.clone(),
        hold_timeout: DEFAULT_HOLD_TIMEOUT,
        sink: with_sink.then(|| sink.clone() as Arc<dyn ViolationSink>),
        view: view.clone(),
    }));
    Harness {
        decider,
        sink,
        clock,
        mode: view.mode.clone(),
        view,
    }
}

/// The card `hold_id` answered with `answer`, judged against the lists as the view has them
/// now (what the sandbox reads off every lock before it settles); whether the hold was on the
/// books.
fn settle(harness: &Harness, hold_id: &str, answer: HoldAnswer) -> bool {
    let domains = harness.view.domains.lock().clone();
    harness
        .decider
        .settle_hold(hold_id, answer, &domains)
        .is_some()
}

fn net_grant(pattern: &str, port: Option<u16>, decision: GrantDecision, expires: Expiry) -> Grant {
    Grant {
        id: GrantId::new(format!("{pattern}-{port:?}-{decision:?}")),
        subject: GrantSubject::NetHost {
            host: HostPattern::new(pattern),
            port,
        },
        scope: GrantScope::Session,
        expires,
        decision,
        granted_at: NOW,
        granted_by: "cli".to_owned(),
        via: None,
    }
}

fn origin(value: &str) -> WebsiteOrigin {
    WebsiteOrigin::parse(value).unwrap()
}

async fn decide(harness: &Harness, value: &str) -> Decision {
    harness.decider.decide(&origin(value), None).await
}

fn is_allow(decision: &Decision) -> bool {
    matches!(decision, Decision::Allow)
}

fn is_deny(decision: &Decision) -> bool {
    matches!(decision, Decision::Deny)
}

/// What the proxy does with an `Ask` once its hold budget admits the connection: park it.
async fn hold(
    harness: &Harness,
    origin: &WebsiteOrigin,
    call: Option<&CommandTag>,
) -> oneshot::Receiver<bool> {
    match harness.decider.decide(origin, call).await {
        Decision::Ask => harness.decider.hold(origin, call).await,
        other => panic!("expected an ask, got {other:?}"),
    }
}

async fn ask(harness: &Harness, value: &str) -> oneshot::Receiver<bool> {
    hold(harness, &origin(value), None).await
}

/// What the proxy hears back for a parked connection, within the test's patience.
async fn answered(
    receiver: oneshot::Receiver<bool>,
) -> Result<Result<bool, oneshot::error::RecvError>, tokio::time::error::Elapsed> {
    tokio::time::timeout(Duration::from_secs(5), receiver).await
}

/// Every card posted for `host`, grantable and informational alike.
fn cards_for(sink: &RecordingSink, host: &str) -> usize {
    sink.posted
        .lock()
        .iter()
        .filter(|(_, violation)| {
            matches!(&violation.blocked, Blocked::Net { host: Some(h), .. } if h == host)
        })
        .count()
}

/// The verdict an observe-mode decision recorded; panics on anything but `AllowAndRecord`.
fn recorded(decision: Decision) -> WouldBe {
    match decision {
        Decision::AllowAndRecord { would } => would,
        Decision::Allow => panic!("expected observe to record, got a plain allow"),
        Decision::Deny => panic!("observe never blocks"),
        Decision::Ask => panic!("observe never asks"),
    }
}

#[tokio::test]
async fn deny_row_beats_allow_row_and_web_fetch_allowlist() {
    let harness = harness(SandboxMode::Enforce, true).await;
    harness.view.set_domains(&["blocked.example"], &[]);
    harness.view.set_rows([
        net_grant("blocked.example", None, GrantDecision::Allow, Expiry::Never),
        net_grant("*.example", None, GrantDecision::Deny, Expiry::Never),
    ]);
    assert!(is_deny(&decide(&harness, "https://blocked.example").await));
    assert!(harness.sink.posted.lock().is_empty());
}

/// The remembered `disallowed_web_fetch_domains` beats the card's `*` allow row: both deny
/// sources are read before any allow. The refusal is visible — one informational `policy_denylist` card per
/// `(call, host, port)` while its deadline runs, nothing proposed, nothing parked.
#[tokio::test]
async fn disallowed_web_fetch_domain_beats_an_allow_all_row_and_posts_one_informational_card() {
    let harness = harness(SandboxMode::Enforce, true).await;
    harness.view.set_domains(&[], &["tracker.example"]);
    assert!(is_deny(&decide(&harness, "https://tracker.example").await));
    harness
        .view
        .set_rows([net_grant("*", None, GrantDecision::Allow, Expiry::Never)]);
    assert!(is_deny(&decide(&harness, "https://tracker.example").await));
    assert!(is_deny(
        &decide(&harness, "https://cdn.tracker.example").await
    ));
    let call = CommandTag::for_call(&CallId::tool("call-tracked"));
    assert!(is_deny(
        &harness
            .decider
            .decide(&origin("https://tracker.example"), Some(&call))
            .await
    ));
    assert!(is_allow(&decide(&harness, "https://other.example").await));

    assert!(harness.sink.hold_ids().is_empty(), "nothing is parked");
    let denylist = InformationalReason::PolicyDenylist;
    assert_eq!(
        vec![
            (None, "tracker.example:443".to_owned(), denylist),
            (None, "cdn.tracker.example:443".to_owned(), denylist),
            (
                Some(call.clone()),
                "tracker.example:443".to_owned(),
                denylist
            ),
        ],
        harness.sink.informational()
    );
    let (_, card) = harness.sink.posted.lock().first().cloned().unwrap();
    assert_eq!(None, card.proposed);
    assert!(matches!(card.replay, Replay::Resume { .. }));
    assert!(!settle(
        &harness,
        &match card.replay {
            Replay::Resume { hold_id } => hold_id,
            Replay::Rerun => unreachable!(),
        },
        HoldAnswer::Allow
    ));

    // Past the card's deadline a fresh hit raises the card again
    harness
        .clock
        .advance(i64::try_from(DEFAULT_HOLD_TIMEOUT.as_secs()).unwrap());
    assert!(is_deny(&decide(&harness, "https://tracker.example").await));
    assert_eq!(4, harness.sink.informational().len());
}

/// A host that leaves the deny-list while its informational card is still on screen is asked
/// about on the next connection: the ask raises its own card, which settles, instead of waiting
/// under the informational one until the proxy gives up.
#[tokio::test]
async fn an_ask_after_a_host_leaves_the_denylist_raises_a_card_that_settles() {
    let harness = harness(SandboxMode::Enforce, true).await;
    harness.view.set_domains(&[], &["tracker.example"]);
    assert!(is_deny(&decide(&harness, "https://tracker.example").await));
    assert_eq!(1, harness.sink.informational().len());

    harness.view.set_domains(&[], &[]);
    let receiver = ask(&harness, "https://tracker.example").await;
    let holds = harness.sink.hold_ids();
    let [hold_id] = holds.as_slice() else {
        panic!("one grantable card: {holds:?}");
    };
    assert!(settle(&harness, hold_id, HoldAnswer::Allow));
    assert_eq!(Ok(true), receiver.await);
}

/// A row recorded between the decision and the park (another hold's answer covering this origin)
/// answers the connection at once: no card is raised for an origin that already has a row.
#[tokio::test]
async fn a_row_recorded_after_the_decision_answers_the_park_without_a_card() {
    let harness = harness(SandboxMode::Enforce, true).await;
    for (decision, answer) in [(GrantDecision::Allow, true), (GrantDecision::Deny, false)] {
        let target = origin("https://race.example");
        assert!(matches!(
            harness.decider.decide(&target, None).await,
            Decision::Ask
        ));
        harness
            .view
            .set_rows([net_grant("race.example", None, decision, Expiry::Never)]);
        let receiver = harness.decider.hold(&target, None).await;
        let answered = tokio::time::timeout(Duration::from_secs(5), receiver).await;
        assert_eq!(Ok(Ok(answer)), answered, "{decision:?}: answered at once");
        harness.view.set_rows([]);
    }
    assert!(harness.sink.hold_ids().is_empty(), "no card was raised");
}

/// Who asks: the session token; a call whose spawn record pins a mode; a known tag the table
/// holds no mode for, alone or beside a call running under `enforce`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Caller {
    Session,
    Pinned(SandboxMode),
    Unrecorded,
    UnrecordedBesideEnforced,
}

const CALLERS: [Caller; 6] = [
    Caller::Session,
    Caller::Pinned(SandboxMode::Enforce),
    Caller::Pinned(SandboxMode::Observe),
    Caller::Pinned(SandboxMode::Off),
    Caller::Unrecorded,
    Caller::UnrecordedBesideEnforced,
];

/// Where the folder's mode flips: each await the decision path once re-read the mode across,
/// and the two gaps around the park.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FlipAt {
    /// During `decide`'s lists read.
    DecideLists,
    /// While `decide` posts the deny-list card.
    DenylistPost,
    /// Between `decide` returning `Ask` and the park.
    BeforePark,
    /// During the park's lists read.
    ParkLists,
    /// While the park posts the card.
    CardPost,
    /// With the card up, before its answer.
    CardUp,
}

/// The origin asked about: one nothing covers, or one on the deny list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Unknown,
    Denylisted,
}

/// Every flip point with the target that reaches it.
const FLIPS: [(FlipAt, Target); 7] = [
    (FlipAt::DecideLists, Target::Unknown),
    (FlipAt::DecideLists, Target::Denylisted),
    (FlipAt::DenylistPost, Target::Denylisted),
    (FlipAt::BeforePark, Target::Unknown),
    (FlipAt::ParkLists, Target::Unknown),
    (FlipAt::CardPost, Target::Unknown),
    (FlipAt::CardUp, Target::Unknown),
];

const MODES: [SandboxMode; 3] = [SandboxMode::Enforce, SandboxMode::Observe, SandboxMode::Off];

/// What the proxy's receiver says once the park returned.
#[derive(Debug, PartialEq, Eq)]
enum Parked {
    Never,
    Answered(bool),
    Refused,
    Waiting,
}

fn parked(receiver: Option<&mut oneshot::Receiver<bool>>) -> Parked {
    match receiver.map(oneshot::Receiver::try_recv) {
        None => Parked::Never,
        Some(Ok(answer)) => Parked::Answered(answer),
        Some(Err(oneshot::error::TryRecvError::Closed)) => Parked::Refused,
        Some(Err(oneshot::error::TryRecvError::Empty)) => Parked::Waiting,
    }
}

/// Everything one connection's caller and user saw.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    decision: String,
    parked: Parked,
    cards: usize,
    informational: usize,
    /// The card's `Allow` settles it, and what the receiver then says.
    settled: Option<(bool, Parked)>,
}

/// One connection from `caller` under a folder at `folder`, flipped to `to` at `at` if any.
async fn connection(
    caller: Caller,
    folder: SandboxMode,
    flip: Option<(FlipAt, SandboxMode)>,
    target: Target,
) -> Outcome {
    let harness = harness(folder, true).await;
    let tag = CommandTag::for_call(&CallId::tool("table"));
    let call = match caller {
        Caller::Session => None,
        Caller::Pinned(mode) => {
            harness.view.pin(&tag, mode);
            Some(&tag)
        }
        Caller::Unrecorded => {
            harness.view.enter(&tag, None);
            Some(&tag)
        }
        Caller::UnrecordedBesideEnforced => {
            harness.view.enter(&tag, None);
            harness.view.pin(
                &CommandTag::for_call(&CallId::tool("enforced")),
                SandboxMode::Enforce,
            );
            Some(&tag)
        }
    };
    let origin = origin("https://table.example");
    if target == Target::Denylisted {
        harness.view.set_domains(&[], &["table.example"]);
    }
    let flip_to = |to: SandboxMode| {
        let mode = harness.mode.clone();
        Box::new(move || *mode.lock() = to) as Box<dyn FnOnce() + Send>
    };
    match flip {
        Some((FlipAt::DecideLists, to)) => harness.view.flip_during_lists_read(0, to),
        Some((FlipAt::ParkLists, to)) => harness.view.flip_during_lists_read(1, to),
        Some((FlipAt::DenylistPost | FlipAt::CardPost, to)) => {
            *harness.sink.during_post.lock() = Some(flip_to(to));
        }
        Some((FlipAt::BeforePark | FlipAt::CardUp, _)) | None => {}
    }
    let decision = harness.decider.decide(&origin, call).await;
    let mut receiver = match decision {
        Decision::Ask => {
            if let Some((FlipAt::BeforePark, to)) = flip {
                *harness.mode.lock() = to;
            }
            Some(harness.decider.hold(&origin, call).await)
        }
        _ => None,
    };
    if let Some((FlipAt::CardUp, to)) = flip {
        *harness.mode.lock() = to;
    }
    let parked_now = parked(receiver.as_mut());
    let settled = harness.sink.hold_ids().first().map(|id| {
        let settled = settle(&harness, id, HoldAnswer::Allow);
        (settled, parked(receiver.as_mut()))
    });
    Outcome {
        decision: format!("{decision:?}"),
        parked: parked_now,
        cards: harness.sink.hold_ids().len(),
        informational: harness.sink.informational().len(),
        settled,
    }
}

/// The mode `caller`'s connection is decided under: its own for a pinned call; `enforce` for a
/// tag with no mode while an enforced call runs; else the folder's as read once after the
/// lists, so a flip during that read is seen and one anywhere later is not.
fn expected_mode(caller: Caller, folder: SandboxMode, flip: (FlipAt, SandboxMode)) -> SandboxMode {
    match (caller, flip) {
        (Caller::Pinned(mode), _) => mode,
        (Caller::UnrecordedBesideEnforced, _) => SandboxMode::Enforce,
        (Caller::Session | Caller::Unrecorded, (FlipAt::DecideLists, to)) => to,
        (Caller::Session | Caller::Unrecorded, _) => folder,
    }
}

/// `mode_for`'s invariants, over every caller × folder mode × flip target × flip point: a
/// connection under a flip ends exactly as a session-token connection under the mode the
/// caller is owed does with no flip at all. A pinned call is owed its own mode whatever the
/// folder does and whenever it does it; a tag without a mode is owed `enforce` while an
/// enforced call runs; the session token and a lone tag without a mode are owed the folder's
/// mode as read once after the lists — a flip during that read is applied, one at any later
/// point is not, and no flip anywhere releases, refuses or cards what the decision did not.
#[tokio::test]
async fn every_caller_is_decided_under_the_mode_it_is_owed_whenever_the_folder_flips() {
    let mut checked = 0;
    for caller in CALLERS {
        for folder in MODES {
            for to in MODES.into_iter().filter(|to| *to != folder) {
                for (at, target) in FLIPS {
                    let owed = expected_mode(caller, folder, (at, to));
                    let flipped = connection(caller, folder, Some((at, to)), target).await;
                    let reference = connection(Caller::Session, owed, None, target).await;
                    assert_eq!(
                        reference, flipped,
                        "{caller:?} under {folder:?} flipped to {to:?} at {at:?} ({target:?}) \
                         is decided as the session under {owed:?}"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(CALLERS.len() * MODES.len() * 2 * FLIPS.len(), checked);
}

/// A call is decided under its own mode, whatever the folder's: an `enforce` call is asked
/// under an observing (or off) folder, an `observe` call records under an enforcing folder, an
/// `off` call is allowed under either — and a flip after the decision leaves the enforce
/// call's hold parked until its own card answers it.
#[tokio::test]
async fn tagged_call_is_decided_under_its_own_mode_whatever_the_folder_does() {
    let harness = harness(SandboxMode::Observe, true).await;
    let enforced = CommandTag::for_call(&CallId::tool("enforced"));
    let observed = CommandTag::for_call(&CallId::tool("observed"));
    let off = CommandTag::for_call(&CallId::tool("off"));
    harness.view.pin(&enforced, SandboxMode::Enforce);
    harness.view.pin(&observed, SandboxMode::Observe);
    harness.view.pin(&off, SandboxMode::Off);
    let target = origin("https://own-mode.example");
    for folder in MODES {
        *harness.mode.lock() = folder;
        assert!(
            matches!(
                harness.decider.decide(&target, Some(&enforced)).await,
                Decision::Ask
            ),
            "{folder:?}: the enforce call is asked"
        );
        assert_eq!(
            WouldBe::Asked,
            recorded(harness.decider.decide(&target, Some(&observed)).await),
            "{folder:?}: the observe call records"
        );
        assert!(
            is_allow(&harness.decider.decide(&target, Some(&off)).await),
            "{folder:?}: the off call is allowed"
        );
    }
    let mut parked = harness.decider.hold(&target, Some(&enforced)).await;
    let [card]: [String; 1] = harness.sink.hold_ids().try_into().unwrap();
    for folder in MODES {
        *harness.mode.lock() = folder;
        assert!(
            harness.decider.is_pending(&card),
            "{folder:?}: the card stays up"
        );
        assert!(parked.try_recv().is_err(), "{folder:?}: still parked");
    }
    assert!(settle(&harness, &card, HoldAnswer::Deny));
    assert_eq!(Ok(false), parked.await, "only its own card answers it");
}

/// A session-token decision reads the folder's mode once, after the lists: a flip that lands
/// during the lists read is what the decision applies, in either direction.
#[tokio::test]
async fn session_token_mode_is_read_once_after_the_lists() {
    let harness = harness(SandboxMode::Enforce, true).await;
    harness.view.flip_during_lists_read(0, SandboxMode::Observe);
    assert_eq!(
        WouldBe::Asked,
        recorded(decide(&harness, "https://during.example").await),
        "the flip to observe during the lists read is applied"
    );
    harness.view.flip_during_lists_read(0, SandboxMode::Enforce);
    assert!(
        matches!(
            decide(&harness, "https://during.example").await,
            Decision::Ask
        ),
        "the flip to enforce during the lists read is applied"
    );
    assert!(harness.sink.posted.lock().is_empty());
}

/// A credential whose call the table holds no mode for is decided as a token-less request: the
/// folder's mode while the proxy admits the unauthenticated, `enforce` — with nobody to ask —
/// while it does not, because an enforced call runs. It never gets more than dropping its
/// token would.
#[tokio::test]
async fn tag_without_a_mode_is_decided_as_a_token_less_request() {
    let harness = harness(SandboxMode::Observe, true).await;
    let bare = CommandTag::for_call(&CallId::tool("bare"));
    let enforced = CommandTag::for_call(&CallId::tool("enforced"));
    harness.view.enter(&bare, None);
    let target = origin("https://bare.example");
    assert_eq!(
        WouldBe::Asked,
        recorded(harness.decider.decide(&target, Some(&bare)).await),
        "alone, it follows the observing folder"
    );
    harness.view.pin(&enforced, SandboxMode::Enforce);
    assert!(
        matches!(
            harness.decider.decide(&target, Some(&bare)).await,
            Decision::Ask
        ),
        "beside an enforced call the folder admits nothing unauthenticated: enforce"
    );
    *harness.mode.lock() = SandboxMode::Off;
    assert!(matches!(
        harness.decider.decide(&target, Some(&bare)).await,
        Decision::Ask
    ));
    harness.view.unpin(&enforced);
    assert!(
        is_allow(&harness.decider.decide(&target, Some(&bare)).await),
        "the enforced call ended: the folder's mode again"
    );
}

/// A row recorded between the decision and the park answers under the lock with nothing put
/// on the books: it counts against no card budget and nothing joins it.
#[tokio::test]
async fn row_answered_park_puts_nothing_on_the_books() {
    let harness = harness(SandboxMode::Enforce, true).await;
    for n in 0..MAX_PENDING_CARDS - 1 {
        drop(ask(&harness, &format!("https://h{n}.budget.example")).await);
    }
    let target = origin("https://judged.example");
    assert!(matches!(
        harness.decider.decide(&target, None).await,
        Decision::Ask
    ));
    harness.view.set_rows([net_grant(
        "judged.example",
        None,
        GrantDecision::Deny,
        Expiry::Never,
    )]);
    assert_eq!(
        Ok(Ok(false)),
        answered(harness.decider.hold(&target, None).await).await
    );
    harness.view.set_rows([]);
    let mut asked = ask(&harness, "https://judged.example").await;
    assert!(
        asked.try_recv().is_err(),
        "the next ask waits on its own card"
    );
    assert_eq!(
        MAX_PENDING_CARDS,
        harness.sink.hold_ids().len(),
        "the refused connection took no card from the budget"
    );
}

/// A `permission.toml` edit between the decision and the park is judged at the park like a row:
/// a host that joined the allow list is let through without a card, one that joined the deny
/// list is refused with the informational card the decision would have posted.
#[tokio::test]
async fn a_list_entry_added_after_the_decision_answers_the_park() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let target = origin("https://listed.example");
    assert!(matches!(
        harness.decider.decide(&target, None).await,
        Decision::Ask
    ));
    harness.view.set_domains(&["listed.example"], &[]);
    let receiver = harness.decider.hold(&target, None).await;
    assert_eq!(
        Ok(Ok(true)),
        tokio::time::timeout(Duration::from_secs(5), receiver).await
    );
    assert!(
        harness.sink.posted.lock().is_empty(),
        "no card for an allowed host"
    );

    harness.view.set_domains(&[], &[]);
    assert!(matches!(
        harness.decider.decide(&target, None).await,
        Decision::Ask
    ));
    harness.view.set_domains(&[], &["listed.example"]);
    let receiver = harness.decider.hold(&target, None).await;
    let answered = tokio::time::timeout(Duration::from_secs(5), receiver).await;
    assert!(
        matches!(answered, Ok(Err(_))),
        "the deny list refuses at the park: {answered:?}"
    );
    assert!(harness.sink.hold_ids().is_empty(), "no grantable card");
    assert_eq!(
        vec![(
            None,
            "listed.example:443".to_owned(),
            InformationalReason::PolicyDenylist
        )],
        harness.sink.informational()
    );
}

/// A card is on the decider's books from its park until it is settled, by its own answer or by a
/// row that now covers it; one a covering row released is no longer pending, so the sink drops
/// its post instead of showing a card for an answered connection.
#[tokio::test]
async fn a_hold_is_pending_until_a_covering_row_settles_it() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let target = origin("https://pending.example");
    let receiver = harness.decider.hold(&target, None).await;
    let hold_id = harness
        .sink
        .hold_ids()
        .first()
        .cloned()
        .expect("card posted");
    assert!(harness.decider.is_pending(&hold_id));
    harness.view.set_rows([net_grant(
        "pending.example",
        None,
        GrantDecision::Allow,
        Expiry::Never,
    )]);
    assert!(!settle(&harness, "another-card", HoldAnswer::Allow));
    let answered = tokio::time::timeout(Duration::from_secs(5), receiver).await;
    assert_eq!(Ok(Ok(true)), answered);
    assert!(!harness.decider.is_pending(&hold_id));
}

/// A mode change releases nothing: a connection parked under `enforce` stays parked and its
/// card up through a flip to `observe`, to `off` and back, until its own answer (or a covering
/// row, its call's end, or the proxy's stop) ends it.
#[tokio::test]
async fn mode_change_releases_nothing_parked() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let mut parked = ask(&harness, "https://parked.example").await;
    let [card]: [String; 1] = harness.sink.hold_ids().try_into().unwrap();
    for folder in [SandboxMode::Observe, SandboxMode::Off, SandboxMode::Enforce] {
        *harness.mode.lock() = folder;
        assert!(
            harness.decider.is_pending(&card),
            "{folder:?}: the card stays up"
        );
        assert!(
            matches!(parked.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
            "{folder:?}: still parked"
        );
    }
    assert!(settle(&harness, &card, HoldAnswer::Allow));
    assert_eq!(Ok(true), parked.await);
    assert!(!harness.decider.is_pending(&card));
}

/// A hold released by anything but its own card's answer — a row another card recorded, its
/// call's epoch ending, the proxy stopping, its card's deadline — withdraws that card: its
/// token is cancelled, so the task waiting on the user ends and a late answer records nothing.
/// The hold's own answer leaves its token alone (that task is the one answering).
#[tokio::test]
async fn a_hold_released_by_anything_but_its_own_answer_withdraws_its_card() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let ending = CommandTag::for_call(&CallId::tool("ending"));
    harness.view.pin(&ending, SandboxMode::Enforce);
    let answered = harness
        .decider
        .hold(&origin("https://answered.example"), None)
        .await;
    let covered = harness
        .decider
        .hold(&origin("https://covered.example"), None)
        .await;
    let ended = harness
        .decider
        .hold(&origin("https://ended.example"), Some(&ending))
        .await;
    let ids = harness.sink.hold_ids();
    let tokens: Vec<_> = ids
        .iter()
        .map(|id| harness.decider.card_token(id).expect("on the books"))
        .collect();
    let [answered_token, covered_token, ended_token] = tokens.as_slice() else {
        panic!("three cards: {ids:?}");
    };
    assert!(tokens.iter().all(|token| !token.is_cancelled()));

    // Its own answer records a row covering the second hold: that one's card is withdrawn
    harness.view.set_rows([net_grant(
        "covered.example",
        None,
        GrantDecision::Allow,
        Expiry::Never,
    )]);
    assert!(settle(&harness, ids.first().unwrap(), HoldAnswer::Allow));
    assert_eq!(
        Ok(Ok(true)),
        tokio::time::timeout(Duration::from_secs(5), answered).await
    );
    assert!(
        !answered_token.is_cancelled(),
        "the hold's own answer is not a withdrawal"
    );
    assert!(
        harness.decider.card_token(ids.first().unwrap()).is_none(),
        "off the books"
    );
    assert_eq!(
        Ok(Ok(true)),
        tokio::time::timeout(Duration::from_secs(5), covered).await
    );
    assert!(covered_token.is_cancelled(), "the row withdrew its card");
    assert!(!ended_token.is_cancelled());

    harness.decider.release_call_holds(&ending, 1);
    assert_eq!(
        Ok(Ok(false)),
        tokio::time::timeout(Duration::from_secs(5), ended).await,
        "the call's end refuses what it still held"
    );
    assert!(
        ended_token.is_cancelled(),
        "the call's end withdrew its card"
    );

    let stopped = harness
        .decider
        .hold(&origin("https://stopped.example"), None)
        .await;
    let stopped_token = harness
        .decider
        .card_token(harness.sink.hold_ids().last().unwrap())
        .expect("on the books");
    harness.decider.release_all(HoldAnswer::Deny);
    assert_eq!(
        Ok(Ok(false)),
        tokio::time::timeout(Duration::from_secs(5), stopped).await
    );
    assert!(stopped_token.is_cancelled(), "the stop withdrew its card");

    // A card nobody waits on any more is withdrawn once its deadline has passed
    let abandoned = harness
        .decider
        .hold(&origin("https://abandoned.example"), None)
        .await;
    let abandoned_token = harness
        .decider
        .card_token(harness.sink.hold_ids().last().unwrap())
        .expect("on the books");
    drop(abandoned);
    harness
        .clock
        .advance(i64::try_from(DEFAULT_HOLD_TIMEOUT.as_secs()).unwrap() + 1);
    // A later hold sweeps the entries past their deadline; its own receiver is not needed
    drop(
        harness
            .decider
            .hold(&origin("https://later.example"), None)
            .await,
    );
    assert!(
        abandoned_token.is_cancelled(),
        "past its deadline the card is withdrawn"
    );
}

/// A stopping proxy releases every parked connection with the given answer, so its shutdown
/// never waits out a hold.
#[tokio::test]
async fn release_all_answers_every_parked_connection() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let parked = harness
        .decider
        .hold(&origin("https://stopping.example"), None)
        .await;
    harness.decider.release_all(HoldAnswer::Deny);
    let answered = tokio::time::timeout(Duration::from_secs(5), parked).await;
    assert_eq!(Ok(Ok(false)), answered);
    assert!(
        harness
            .sink
            .hold_ids()
            .iter()
            .all(|id| !harness.decider.is_pending(id))
    );
}

/// A stop that lands while a connection is between its decision and its park finds the books
/// closed at the park: the connection is refused under the lock, no card is posted and nothing
/// is left pending for a proxy that is gone. A deny-list hit decided after the stop posts no
/// informational card either. The stop lands in the park's lists read, the one await between
/// `decide` and the insert.
#[tokio::test]
async fn stop_landing_between_the_decision_and_the_park_posts_no_card_and_leaves_nothing_pending() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let decider = harness.decider.clone();
    // Read 0 is the decision's lists read, read 1 the park's
    harness
        .view
        .run_during_lists_read(1, move || decider.stopping());
    let parked = ask(&harness, "https://stopping.example").await;
    assert!(
        matches!(answered(parked).await, Ok(Err(_))),
        "refused: the receiver is closed, not left waiting on a card"
    );
    assert!(
        harness.sink.posted.lock().is_empty(),
        "no card for a stopped proxy"
    );
    assert!(
        harness.decider.pending.lock().holds.is_empty(),
        "nothing on the books"
    );

    harness.view.set_domains(&[], &["listed.example"]);
    assert!(is_deny(&decide(&harness, "https://listed.example").await));
    assert!(
        harness.sink.informational().is_empty(),
        "a deny-list hit after the stop raises no card"
    );
    assert!(harness.decider.pending.lock().holds.is_empty());
}

/// A call whose spawn ends between the decision and the park (its child exited; this connection
/// authenticated before that) is refused at the park: the receiver is closed — the proxy's deny —
/// no card is posted and nothing is booked. A hold parked while the spawn ran is left alone.
#[tokio::test]
async fn a_spawn_ending_between_the_decision_and_the_park_refuses_the_connection_without_a_card() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let call = CommandTag::for_call(&CallId::tool("exiting"));
    harness.view.pin(&call, SandboxMode::Enforce);
    let mut earlier = hold(&harness, &origin("https://earlier.example"), Some(&call)).await;
    assert_eq!(1, harness.sink.hold_ids().len());

    let view = harness.view.clone();
    let exiting = call.clone();
    // Read 0 is the decision's lists read, read 1 the park's
    harness
        .view
        .run_during_lists_read(1, move || view.end_spawn(&exiting));
    let late = hold(&harness, &origin("https://late.example"), Some(&call)).await;
    assert!(
        matches!(answered(late).await, Ok(Err(_))),
        "refused: the receiver is closed, not left waiting on a card"
    );
    assert_eq!(
        1,
        harness.sink.hold_ids().len(),
        "no card for a connection nothing runs to answer"
    );
    assert_eq!(0, cards_for(&harness.sink, "late.example"));
    assert_eq!(
        1,
        harness.decider.pending.lock().holds.len(),
        "nothing booked for it; the earlier hold is the exit's drain to refuse"
    );
    assert!(earlier.try_recv().is_err(), "still parked");

    harness.decider.release_call_holds(&call, 1);
    assert_eq!(Ok(Ok(false)), answered(earlier).await);
    assert!(harness.decider.pending.lock().holds.is_empty());
}

/// A single-port deny under an all-ports allow still refuses that port: the decider judges each
/// connection from every host row, a deny first, whatever `live_allows` keeps for the file-system
/// policy.
#[tokio::test]
async fn a_port_deny_refuses_its_port_under_an_all_ports_allow() {
    let harness = harness(SandboxMode::Enforce, true).await;
    harness.view.set_rows([
        net_grant("ports.example", None, GrantDecision::Allow, Expiry::Never),
        net_grant(
            "ports.example",
            Some(443),
            GrantDecision::Deny,
            Expiry::Never,
        ),
    ]);
    assert!(matches!(
        harness
            .decider
            .decide(&origin("https://ports.example"), None)
            .await,
        Decision::Deny
    ));
    assert!(matches!(
        harness
            .decider
            .decide(&origin("http://ports.example"), None)
            .await,
        Decision::Allow
    ));
}

/// Only `enforce` makes the proxy authenticate: under `observe` a request whose credential was
/// revoked (a background process after its call) or never given is decided as the session's.
#[tokio::test]
async fn only_enforce_refuses_unauthenticated_requests() {
    let harness = harness(SandboxMode::Enforce, true).await;
    assert!(!harness.decider.admits_unauthenticated());
    *harness.mode.lock() = SandboxMode::Observe;
    assert!(harness.decider.admits_unauthenticated());
}

/// A call running under `enforce` after the folder's flip to `observe` keeps the proxy
/// authenticating for everyone: a request with no credential is refused while that call runs
/// (its own command could be the one that dropped the token), and admitted again once it has
/// ended. Under `off` the same rule holds — the running call's mode is what decides; a call
/// running under `observe` or a known tag with no mode changes nothing.
#[tokio::test]
async fn nothing_unauthenticated_is_admitted_while_any_call_runs_enforced() {
    let harness = harness(SandboxMode::Observe, true).await;
    assert!(harness.decider.admits_unauthenticated());
    let observed = CommandTag::for_call(&CallId::tool("observed"));
    let bare = CommandTag::for_call(&CallId::tool("bare"));
    harness.view.pin(&observed, SandboxMode::Observe);
    harness.view.enter(&bare, None);
    assert!(harness.decider.admits_unauthenticated());
    let enforced = CommandTag::for_call(&CallId::tool("enforced"));
    harness.view.pin(&enforced, SandboxMode::Enforce);
    assert!(
        !harness.decider.admits_unauthenticated(),
        "an enforced call is running: the observing folder still refuses a credential-less request"
    );
    *harness.mode.lock() = SandboxMode::Off;
    assert!(!harness.decider.admits_unauthenticated());
    harness.view.unpin(&enforced);
    assert!(harness.decider.admits_unauthenticated());
}

/// A call's epoch ending refuses the holds parked under it and under earlier epochs — their
/// cards withdrawn — and leaves what a later epoch parked (the background child's connection,
/// parked after the detach moved the epoch on), another call's hold and the session's alone.
#[tokio::test]
async fn ended_epoch_refuses_its_holds_and_leaves_later_epochs_parked() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let call = CommandTag::for_call(&CallId::tool("epochs"));
    let other = CommandTag::for_call(&CallId::tool("other"));
    harness.view.pin(&call, SandboxMode::Enforce);
    harness.view.pin(&other, SandboxMode::Enforce);
    let first = hold(&harness, &origin("https://first.example"), Some(&call)).await;
    let bumped = harness.view.bump_epoch(&call);
    assert_eq!(2, bumped);
    let mut second = hold(&harness, &origin("https://second.example"), Some(&call)).await;
    let mut theirs = hold(&harness, &origin("https://first.example"), Some(&other)).await;
    let mut session = ask(&harness, "https://first.example").await;
    let [first_id, second_id, theirs_id, session_id]: [String; 4] =
        harness.sink.hold_ids().try_into().unwrap();
    let first_token = harness.decider.card_token(&first_id).unwrap();

    harness.decider.release_call_holds(&call, 1);
    assert_eq!(Ok(Ok(false)), answered(first).await, "refused");
    assert!(first_token.is_cancelled(), "its card is withdrawn");
    assert!(!harness.decider.is_pending(&first_id));
    for (name, id, receiver) in [
        ("the later epoch's", &second_id, &mut second),
        ("the other call's", &theirs_id, &mut theirs),
        ("the session's", &session_id, &mut session),
    ] {
        assert!(harness.decider.is_pending(id), "{name} card stays up");
        assert!(receiver.try_recv().is_err(), "{name} hold stays parked");
    }

    harness.decider.release_call_holds(&call, bumped);
    assert_eq!(Ok(Ok(false)), answered(second).await);
    assert!(harness.decider.is_pending(&theirs_id));
    assert!(harness.decider.is_pending(&session_id));
    assert!(settle(&harness, &theirs_id, HoldAnswer::Allow));
    assert_eq!(Ok(Ok(true)), answered(theirs).await);
}

/// An answer is judged again when it lands: an `Allow` on a card whose origin a deny row or a
/// `disallowed_web_fetch_domains` entry now refuses is applied as a `Deny`, and the caller is
/// told which answer was applied. A `Deny` is applied as given, and so is an `Allow` nothing
/// refuses — an allow row landing meanwhile changes nothing.
#[tokio::test]
async fn allow_answered_after_a_deny_source_landed_is_applied_as_a_deny() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let by_row = ask(&harness, "https://by-row.example").await;
    let by_list = ask(&harness, "https://by-list.example").await;
    let denied = ask(&harness, "https://denied.example").await;
    let plain = ask(&harness, "https://plain.example").await;
    let [by_row_id, by_list_id, denied_id, plain_id]: [String; 4] =
        harness.sink.hold_ids().try_into().unwrap();

    let none = WebFetchDomains::default();
    harness.view.set_rows([net_grant(
        "by-row.example",
        None,
        GrantDecision::Deny,
        Expiry::Never,
    )]);
    assert_eq!(
        Some(HoldAnswer::Deny),
        harness
            .decider
            .settle_hold(&by_row_id, HoldAnswer::Allow, &none)
    );
    assert_eq!(Ok(Ok(false)), answered(by_row).await, "the deny row wins");
    let lists = WebFetchDomains {
        allowed: Default::default(),
        disallowed: ["by-list.example".to_owned()].into_iter().collect(),
    };
    assert_eq!(
        Some(HoldAnswer::Deny),
        harness
            .decider
            .settle_hold(&by_list_id, HoldAnswer::Allow, &lists)
    );
    assert_eq!(Ok(Ok(false)), answered(by_list).await, "the deny list wins");
    assert_eq!(
        Some(HoldAnswer::Deny),
        harness
            .decider
            .settle_hold(&denied_id, HoldAnswer::Deny, &lists)
    );
    assert_eq!(Ok(Ok(false)), answered(denied).await);
    harness.view.set_rows([net_grant(
        "plain.example",
        None,
        GrantDecision::Allow,
        Expiry::Never,
    )]);
    assert_eq!(
        Some(HoldAnswer::Allow),
        harness
            .decider
            .settle_hold(&plain_id, HoldAnswer::Allow, &lists),
        "an allow row refuses nothing"
    );
    assert_eq!(Ok(Ok(true)), answered(plain).await);
}

/// A list entry that lands while cards are up — `permission.toml` edited by hand, reloaded —
/// releases the holds it now covers with its verdict: a host that joined the allow list is let
/// through, one that joined the deny list is refused, and a hold neither list names stays.
#[tokio::test]
async fn list_entries_landing_while_cards_are_up_release_the_holds_they_cover() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let allowed = ask(&harness, "https://allowed.example").await;
    let denied = ask(&harness, "https://cdn.denied.example").await;
    let mut unnamed = ask(&harness, "https://unnamed.example").await;
    harness
        .view
        .set_domains(&["allowed.example"], &["denied.example"]);
    harness
        .decider
        .release_holds_now_covered(&harness.view.domains.lock().clone());
    assert_eq!(Ok(Ok(true)), answered(allowed).await);
    assert_eq!(Ok(Ok(false)), answered(denied).await);
    assert!(
        unnamed.try_recv().is_err(),
        "nothing covers it: still parked"
    );
    let [_, _, unnamed_id]: [String; 3] = harness.sink.hold_ids().try_into().unwrap();
    assert!(harness.decider.is_pending(&unnamed_id));
}

/// The proxy's stop reaches the decider once: every connection still parked is released with
/// a deny and every card those holds raised is withdrawn (its token cancelled, its id off the
/// books), so nothing waits out the listener going away and no late answer lands.
#[tokio::test]
async fn the_proxys_stop_releases_every_hold_with_a_deny_and_withdraws_its_card() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let mut first = ask(&harness, "https://one.example").await;
    let mut second = ask(&harness, "https://two.example").await;
    let ids = harness.sink.hold_ids();
    assert_eq!(2, ids.len());
    let tokens: Vec<_> = ids
        .iter()
        .map(|id| {
            harness
                .decider
                .card_token(id)
                .expect("the card is on the books")
        })
        .collect();
    harness.decider.stopping();
    assert_eq!(Ok(false), first.try_recv(), "released with a deny");
    assert_eq!(Ok(false), second.try_recv(), "released with a deny");
    assert!(tokens.iter().all(|token| token.is_cancelled()));
    assert!(ids.iter().all(|id| !harness.decider.is_pending(id)));
    let first_id = ids.first().expect("two holds were parked");
    assert!(
        !settle(&harness, first_id, HoldAnswer::Allow),
        "a late answer on the withdrawn card settles nothing"
    );
}

/// A park dropped between putting its connection on the books and posting its card — the
/// proxy's hold timeout fired, or its connection task was aborted, while the sink was slow —
/// takes the entry back: a connection that joined it meanwhile is denied instead of waiting
/// under a card nobody sees, and the next ask for the key raises its own card.
#[tokio::test]
async fn a_park_cancelled_before_its_card_is_posted_leaves_nothing_cardless_to_join() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let target = "https://slow-card.example";
    harness.sink.stalled.store(true, Ordering::SeqCst);
    let mut first = Box::pin(ask(&harness, target));
    assert!(
        futures::poll!(first.as_mut()).is_pending(),
        "on the books, parked on the sink's post"
    );
    let joined = ask(&harness, target).await;
    assert_eq!(
        0,
        cards_for(&harness.sink, "slow-card.example"),
        "no card was posted"
    );
    drop(first);
    assert_eq!(
        Ok(Ok(false)),
        answered(joined).await,
        "what joined the taken-back entry is denied"
    );
    harness.sink.stalled.store(false, Ordering::SeqCst);
    let next = ask(&harness, target).await;
    let ids = harness.sink.hold_ids();
    assert_eq!(1, ids.len(), "the next ask raised its own card");
    let id = ids.first().expect("one card was posted");
    assert!(settle(&harness, id, HoldAnswer::Allow));
    assert_eq!(Ok(Ok(true)), answered(next).await);
}

/// A deny-list card is on the books before it is posted; a decision dropped between the two
/// takes it back, so the next connection for the key posts the card instead of finding one
/// "already on screen" that nobody saw.
#[tokio::test]
async fn a_denylist_card_whose_post_was_cancelled_is_raised_by_the_next_connection() {
    let harness = harness(SandboxMode::Enforce, true).await;
    harness.view.set_domains(&[], &["listed.example"]);
    harness.sink.stalled.store(true, Ordering::SeqCst);
    let mut first = Box::pin(decide(&harness, "https://listed.example"));
    assert!(
        futures::poll!(first.as_mut()).is_pending(),
        "the card is on the books, parked on the sink's post"
    );
    drop(first);
    harness.sink.stalled.store(false, Ordering::SeqCst);
    assert!(is_deny(&decide(&harness, "https://listed.example").await));
    assert_eq!(
        1,
        harness.sink.informational().len(),
        "the next connection posted the card"
    );
}

/// The cards one decider keeps on screen are bounded: past [`MAX_PENDING_CARDS`] a new ask is
/// refused without a card and a deny-list hit raises none.
#[tokio::test]
async fn pending_cards_stop_at_the_named_bound() {
    let harness = harness(SandboxMode::Enforce, true).await;
    harness.view.set_domains(&[], &["walk.example"]);
    for n in 0..MAX_PENDING_CARDS + 8 {
        let host = format!("https://h{n}.walk.example");
        assert!(is_deny(&decide(&harness, &host).await));
    }
    assert_eq!(MAX_PENDING_CARDS, harness.sink.informational().len());

    let harness = self::harness(SandboxMode::Enforce, true).await;
    let mut parked = Vec::new();
    for n in 0..MAX_PENDING_CARDS {
        parked.push(ask(&harness, &format!("https://h{n}.ask.example")).await);
    }
    let refused = ask(&harness, "https://one-more.ask.example").await;
    let refused = tokio::time::timeout(Duration::from_secs(5), refused).await;
    assert!(
        matches!(refused, Ok(Err(_))),
        "past the bound the ask is refused at once: {refused:?}"
    );
    assert_eq!(MAX_PENDING_CARDS, harness.sink.hold_ids().len());
    drop(parked);
}

#[tokio::test]
async fn allow_row_matches_its_port_only_and_expires_by_the_clock() {
    let harness = harness(SandboxMode::Enforce, true).await;
    harness.view.set_rows([
        net_grant(
            "api.example",
            Some(443),
            GrantDecision::Allow,
            Expiry::Ttl { seconds: 60 },
        ),
        net_grant(
            "any.example",
            None,
            GrantDecision::Allow,
            Expiry::At { unix: NOW + 5 },
        ),
    ]);
    assert!(is_allow(&decide(&harness, "https://api.example").await));
    assert!(is_allow(&decide(&harness, "http://any.example:8080").await));
    let _other_port = ask(&harness, "http://api.example").await;
    assert_eq!(1, harness.sink.posted.lock().len());

    harness.clock.advance(60);
    let _expired = ask(&harness, "https://api.example").await;
    let _expired_at = ask(&harness, "https://any.example").await;
    assert_eq!(3, harness.sink.posted.lock().len());
}

#[tokio::test]
async fn host_patterns_are_judged_by_host_pattern_matches() {
    let harness = harness(SandboxMode::Enforce, true).await;
    harness.view.set_rows([net_grant(
        "*.npmjs.org",
        None,
        GrantDecision::Allow,
        Expiry::Never,
    )]);
    assert!(is_allow(
        &decide(&harness, "https://registry.npmjs.org").await
    ));
    assert!(is_allow(
        &decide(&harness, "https://Registry.NPMJS.org.").await
    ));
    let _apex = ask(&harness, "https://npmjs.org").await;
    let _lookalike = ask(&harness, "https://notnpmjs.org").await;
    assert_eq!(2, harness.sink.posted.lock().len());
}

/// The view is read at every decision, so a change lands on the next one (the sandbox's view
/// re-reads `permission.toml` when the file changed; the real file is exercised in
/// `sandbox::network_tests`).
#[tokio::test]
async fn web_fetch_allowlist_allows_on_the_next_decision_and_its_denies_win() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let _first = ask(&harness, "https://docs.rs").await;
    harness.view.set_domains(&["docs.rs"], &["tracker.example"]);
    assert!(is_allow(&decide(&harness, "https://docs.rs").await));
    assert!(is_allow(&decide(&harness, "https://www.docs.rs").await));
    assert!(is_deny(&decide(&harness, "https://tracker.example").await));
    assert!(is_deny(
        &decide(&harness, "https://cdn.tracker.example").await
    ));
    let _subdomain = ask(&harness, "https://api.docs.rs").await;
    assert_eq!(2, harness.sink.hold_ids().len());
    assert_eq!(2, harness.sink.informational().len());
}

/// Observe never blocks, deny lists included — a deny row
/// and the organisation's list are recorded as the verdict enforce would have reached, not
/// applied; and no card of either kind is posted.
#[tokio::test]
async fn observe_never_blocks_and_records_the_verdict_enforce_would_reach() {
    let observe = harness(SandboxMode::Observe, true).await;
    assert_eq!(
        WouldBe::Asked,
        recorded(decide(&observe, "https://unknown.example").await)
    );
    observe.view.set_rows([
        net_grant("unknown.example", None, GrantDecision::Deny, Expiry::Never),
        net_grant("allowed.example", None, GrantDecision::Allow, Expiry::Never),
    ]);
    observe.view.set_domains(&["docs.rs"], &["tracker.example"]);
    assert_eq!(
        WouldBe::Denied(DenySource::DenyRow),
        recorded(decide(&observe, "https://unknown.example").await)
    );
    assert_eq!(
        WouldBe::Denied(DenySource::WebFetchDenylist),
        recorded(decide(&observe, "https://cdn.tracker.example").await)
    );
    assert_eq!(
        WouldBe::Allowed,
        recorded(decide(&observe, "https://allowed.example").await)
    );
    assert_eq!(
        WouldBe::Allowed,
        recorded(decide(&observe, "https://docs.rs").await)
    );
    assert!(observe.sink.posted.lock().is_empty());

    let off = harness(SandboxMode::Off, true).await;
    off.view
        .set_rows([net_grant("*", None, GrantDecision::Deny, Expiry::Never)]);
    off.view.set_domains(&[], &["unknown.example"]);
    assert!(is_allow(&decide(&off, "https://unknown.example").await));
    assert!(off.sink.posted.lock().is_empty());
}

/// A hold is keyed by `(call, host, port)`. One command's parallel asks for
/// one origin share a card; another call's — or the session-wide credential's — connection to
/// the same origin is its own card, and a settle releases that hold's waiters only.
#[tokio::test]
async fn holds_are_per_call_and_origin_and_settle_releases_that_holds_waiters_only() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let call = CommandTag::for_call(&CallId::tool("call-9"));
    let other_call = CommandTag::for_call(&CallId::tool("call-10"));
    harness.view.enter(&call, None);
    harness.view.enter(&other_call, None);
    let npm = origin("https://registry.npmjs.org");
    let first = hold(&harness, &npm, Some(&call)).await;
    let second = hold(&harness, &npm, Some(&call)).await;
    let anonymous = ask(&harness, "https://registry.npmjs.org").await;
    let other_port = hold(&harness, &origin("http://registry.npmjs.org"), Some(&call)).await;
    let mut theirs = hold(&harness, &npm, Some(&other_call)).await;

    let posted = harness.sink.posted.lock().clone();
    assert_eq!(4, posted.len(), "one card per (call, host, port)");
    let (posted_call, violation) = posted.first().unwrap();
    assert_eq!(Some(call.clone()), *posted_call);
    assert_eq!(
        Blocked::Net {
            host: Some("registry.npmjs.org".to_owned()),
            port: Some(443),
        },
        violation.blocked
    );
    assert_eq!(
        Some(GrantSubject::NetHost {
            host: HostPattern::new("registry.npmjs.org"),
            port: None,
        }),
        violation.proposed
    );
    assert!(violation.is_grantable());
    assert_eq!(
        vec![Some(call.clone()), None, Some(call), Some(other_call)],
        posted
            .iter()
            .map(|(call, _)| call.clone())
            .collect::<Vec<_>>()
    );

    let [mine_id, anonymous_id, other_port_id, theirs_id]: [String; 4] =
        harness.sink.hold_ids().try_into().unwrap();
    assert!(settle(&harness, &mine_id, HoldAnswer::Allow));
    assert_eq!(Ok(true), first.await);
    assert_eq!(Ok(true), second.await);
    assert!(
        theirs.try_recv().is_err(),
        "the other call's hold is untouched: no row was recorded"
    );
    assert!(!settle(&harness, &mine_id, HoldAnswer::Allow));
    assert!(settle(&harness, &anonymous_id, HoldAnswer::Deny));
    assert_eq!(Ok(false), anonymous.await);
    assert!(settle(&harness, &other_port_id, HoldAnswer::Deny));
    assert_eq!(Ok(false), other_port.await);
    assert!(settle(&harness, &theirs_id, HoldAnswer::Allow));
    assert_eq!(Ok(true), theirs.await);
}

/// After a settle, every other parked hold is re-evaluated against the rows
/// the answer recorded. A workspace allow lifts the other calls' holds on that host explicitly;
/// a workspace deny refuses them; a hold on another host stays parked.
#[tokio::test]
async fn a_settle_that_recorded_a_row_releases_the_other_holds_the_row_now_covers() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let npm = origin("https://registry.npmjs.org");
    let call_a = CommandTag::for_call(&CallId::tool("call-a"));
    let call_b = CommandTag::for_call(&CallId::tool("call-b"));
    harness.view.enter(&call_a, None);
    harness.view.enter(&call_b, None);
    let mine = hold(&harness, &npm, Some(&call_a)).await;
    let theirs = hold(&harness, &npm, Some(&call_b)).await;
    let mut elsewhere = ask(&harness, "https://crates.io").await;
    let [mine_id, theirs_id, elsewhere_id]: [String; 3] =
        harness.sink.hold_ids().try_into().unwrap();

    // The gate recorded a workspace allow row before it settled call-a's hold
    harness.view.set_rows([net_grant(
        "registry.npmjs.org",
        None,
        GrantDecision::Allow,
        Expiry::Never,
    )]);
    assert!(settle(&harness, &mine_id, HoldAnswer::Allow));
    assert_eq!((Ok(true), Ok(true)), (mine.await, theirs.await));
    assert!(
        !settle(&harness, &theirs_id, HoldAnswer::Deny),
        "call-b's hold was released by the row, not by its own card"
    );
    assert!(elsewhere.try_recv().is_err());

    // A deny row recorded by another card's "Always reject" refuses the parked hold on its host
    harness.view.set_rows([net_grant(
        "crates.io",
        None,
        GrantDecision::Deny,
        Expiry::Never,
    )]);
    assert!(!settle(&harness, "some-other-card", HoldAnswer::Deny));
    assert_eq!(Ok(false), elsewhere.await);
    assert!(!settle(&harness, &elsewhere_id, HoldAnswer::Allow));
}

/// Whether a covering row withdraws a card does not depend on unrelated traffic: a hold whose
/// every client hung up (the proxy gave up on them) stays a hold on the books — an unrelated
/// park in between prunes its closed waiters and nothing else — so the row another card records
/// withdraws its card exactly as it would one still waited on, and a late answer settles nothing.
#[tokio::test]
async fn a_covering_row_withdraws_a_hold_whose_clients_hung_up_whatever_ran_in_between() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let hung_up = harness
        .decider
        .hold(&origin("https://hung-up.example"), None)
        .await;
    let [hung_up_id]: [String; 1] = harness.sink.hold_ids().try_into().unwrap();
    let token = harness
        .decider
        .card_token(&hung_up_id)
        .expect("on the books");
    drop(hung_up);
    // Unrelated traffic: another origin's park runs the sweep that prunes closed waiters
    let mut unrelated = ask(&harness, "https://unrelated.example").await;
    assert!(
        harness.decider.is_pending(&hung_up_id),
        "a hold nobody waits on is still the card on screen"
    );

    harness.view.set_rows([net_grant(
        "hung-up.example",
        None,
        GrantDecision::Allow,
        Expiry::Never,
    )]);
    assert!(!settle(&harness, "some-other-card", HoldAnswer::Allow));
    assert!(token.is_cancelled(), "the row withdrew the card");
    assert!(!harness.decider.is_pending(&hung_up_id), "off the books");
    assert!(
        !settle(&harness, &hung_up_id, HoldAnswer::Deny),
        "its late answer settles nothing"
    );
    assert!(unrelated.try_recv().is_err(), "the other origin still asks");
}

/// A call-scoped deny row (the gate records one for "Keep blocked" on a held
/// connection) refuses that call's later connections to the host without a card, and leaves
/// another call's connection to ask.
#[tokio::test]
async fn a_calls_deny_row_refuses_its_later_connections_silently() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let call = CommandTag::for_call(&CallId::tool("call-denied"));
    let other = CommandTag::for_call(&CallId::tool("call-other"));
    harness.view.enter(&call, None);
    harness.view.enter(&other, None);
    let npm = origin("https://registry.npmjs.org");
    let held = hold(&harness, &npm, Some(&call)).await;
    harness.view.call_rows.lock().push((
        call.clone(),
        net_grant(
            "registry.npmjs.org",
            None,
            GrantDecision::Deny,
            Expiry::Never,
        ),
    ));
    assert!(settle(
        &harness,
        harness.sink.hold_ids().first().unwrap(),
        HoldAnswer::Deny
    ));
    assert_eq!(Ok(false), held.await);
    assert!(is_deny(&harness.decider.decide(&npm, Some(&call)).await));
    assert!(is_deny(
        &harness
            .decider
            .decide(&origin("http://registry.npmjs.org"), Some(&call))
            .await
    ));
    let _asked = hold(&harness, &npm, Some(&other)).await;
    assert_eq!(
        2,
        harness.sink.posted.lock().len(),
        "no card for the refused call"
    );
}

/// The proxy's hold timer fired (the receiver dropped) before the user answered: the command's
/// retry re-parks under the card still on screen, and the answer to that card releases it.
#[tokio::test]
async fn a_retry_after_the_proxy_gave_up_reparks_under_the_card_on_screen() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let first = ask(&harness, "https://registry.npmjs.org").await;
    let hold_a = harness.sink.hold_ids().first().cloned().unwrap();
    drop(first);
    let mut second = ask(&harness, "https://registry.npmjs.org").await;
    assert_eq!(vec![hold_a.clone()], harness.sink.hold_ids());
    assert!(second.try_recv().is_err());
    // The user answers the one card there is.
    assert!(settle(&harness, &hold_a, HoldAnswer::Allow));
    assert_eq!(Ok(true), second.await);
    assert!(!settle(&harness, &hold_a, HoldAnswer::Allow));
}

#[tokio::test]
async fn a_card_past_its_deadline_is_forgotten_and_the_retry_raises_a_new_one() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let first = ask(&harness, "https://registry.npmjs.org").await;
    drop(first);
    harness
        .clock
        .advance(i64::try_from(DEFAULT_HOLD_TIMEOUT.as_secs()).unwrap());
    let _second = ask(&harness, "https://registry.npmjs.org").await;
    let [forgotten, fresh]: [String; 2] = harness.sink.hold_ids().try_into().unwrap();
    assert_ne!(forgotten, fresh);
    assert!(!settle(&harness, &forgotten, HoldAnswer::Allow));
    assert!(settle(&harness, &fresh, HoldAnswer::Allow));
}

#[tokio::test]
async fn a_live_hold_is_kept_past_the_deadline_until_it_is_answered() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let mut held = ask(&harness, "https://slow.example").await;
    harness
        .clock
        .advance(i64::try_from(DEFAULT_HOLD_TIMEOUT.as_secs()).unwrap() * 2);
    let mut again = ask(&harness, "https://slow.example").await;
    assert_eq!(1, harness.sink.hold_ids().len());
    assert!(held.try_recv().is_err() && again.try_recv().is_err());
    assert!(settle(
        &harness,
        harness.sink.hold_ids().first().unwrap(),
        HoldAnswer::Deny
    ));
    assert_eq!((Ok(false), Ok(false)), (held.await, again.await));
}

/// A hold's key is case-insensitive in the host, as `HostPattern` is: two spellings of one origin
/// from one call share a card.
#[tokio::test]
async fn one_calls_two_spellings_of_an_origin_share_a_hold() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let https = ask(&harness, "https://Registry.npmjs.org").await;
    let https_again = ask(&harness, "https://registry.npmjs.org").await;
    let http = ask(&harness, "http://registry.npmjs.org").await;
    let [https_id, http_id]: [String; 2] = harness.sink.hold_ids().try_into().unwrap();
    assert!(settle(&harness, &https_id, HoldAnswer::Allow));
    assert_eq!((Ok(true), Ok(true)), (https.await, https_again.await));
    assert!(settle(&harness, &http_id, HoldAnswer::Deny));
    assert_eq!(Ok(false), http.await);
}

/// The decider keeps no memory of an answer — the rows are the memory, and the sandbox records
/// the grant before it settles the hold, so the next connection reads it.
#[tokio::test]
async fn the_next_connection_after_an_allow_reads_the_recorded_row() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let mut first = ask(&harness, "https://crates.io").await;
    let hold = harness.sink.hold_ids().first().cloned().unwrap();
    harness.view.set_rows([net_grant(
        "crates.io",
        None,
        GrantDecision::Allow,
        Expiry::Never,
    )]);
    assert!(settle(&harness, &hold, HoldAnswer::Allow));
    assert_eq!(Ok(true), first.try_recv());
    assert!(is_allow(&decide(&harness, "https://crates.io").await));
    assert_eq!(1, harness.sink.hold_ids().len());
}

#[tokio::test]
async fn a_calls_one_shot_row_covers_that_call_only() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let call = CommandTag::for_call(&CallId::tool("call-once"));
    let other = CommandTag::for_call(&CallId::tool("call-other"));
    harness.view.call_rows.lock().push((
        call.clone(),
        net_grant("once.example", None, GrantDecision::Allow, Expiry::Never),
    ));
    assert!(is_allow(
        &harness
            .decider
            .decide(&origin("https://once.example"), Some(&call))
            .await
    ));
    let _asked = hold(&harness, &origin("https://once.example"), Some(&other)).await;
    let _asked_anonymous = ask(&harness, "https://once.example").await;
    // Each ask reads the rows at the decision and again when it parks, for the same call
    assert_eq!(
        vec![Some(call), Some(other.clone()), Some(other), None, None],
        harness.view.reads.lock().clone()
    );
}

#[tokio::test]
async fn abandoned_and_unknown_holds_settle_false() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let dropped = ask(&harness, "https://gone.example").await;
    drop(dropped);
    harness
        .clock
        .advance(i64::try_from(DEFAULT_HOLD_TIMEOUT.as_secs()).unwrap());
    let _fresh = ask(&harness, "https://gone.example").await;
    let [abandoned, fresh]: [String; 2] = harness.sink.hold_ids().try_into().unwrap();
    assert!(!settle(&harness, &abandoned, HoldAnswer::Allow));
    assert!(settle(&harness, &fresh, HoldAnswer::Allow));
    assert!(!settle(&harness, "no-such-hold", HoldAnswer::Allow));
}

#[tokio::test]
async fn enforce_without_a_sink_denies() {
    let harness = harness(SandboxMode::Enforce, false).await;
    assert!(is_deny(&decide(&harness, "https://unknown.example").await));
}

/// A session-token request follows the folder, read at every decision, so `sandbox.mode.set`
/// reaches the proxy's next session-token ask without a new decider.
#[tokio::test]
async fn session_token_follows_the_folder_at_every_decision() {
    let harness = harness(SandboxMode::Enforce, true).await;
    let _held = ask(&harness, "https://flip.example").await;
    assert_eq!(1, harness.sink.hold_ids().len());

    *harness.mode.lock() = SandboxMode::Observe;
    assert_eq!(SandboxMode::Observe, harness.decider.mode());
    assert_eq!(
        WouldBe::Asked,
        recorded(decide(&harness, "https://flip.example:8443").await)
    );
    *harness.mode.lock() = SandboxMode::Off;
    assert!(is_allow(
        &decide(&harness, "https://flip.example:9443").await
    ));
    assert_eq!(
        1,
        harness.sink.hold_ids().len(),
        "observe and off never post a card"
    );

    *harness.mode.lock() = SandboxMode::Enforce;
    let _held_again = ask(&harness, "https://flip.example:7443").await;
    assert_eq!(2, harness.sink.hold_ids().len());
}

#[tokio::test]
async fn a_short_hold_timeout_forgets_an_abandoned_card_sooner() {
    let sink = Arc::new(RecordingSink::default());
    let clock = Arc::new(FixedClock::at(NOW));
    let decider = SandboxNetworkDecider::new(SandboxNetworkDeciderConfig {
        clock: clock.clone(),
        hold_timeout: Duration::from_secs(5),
        sink: Some(sink.clone()),
        view: Arc::new(StubView::with_mode(SandboxMode::Enforce)),
    });
    let x = origin("https://x.example");
    let first = decider.hold(&x, None).await;
    drop(first);
    clock.advance(4);
    let _same_card = decider.hold(&x, None).await;
    assert_eq!(1, sink.hold_ids().len());
    clock.advance(1);
    let _new_card = decider.hold(&x, None).await;
    assert_eq!(1, sink.hold_ids().len(), "a live waiter keeps the card");
    assert!(
        decider
            .settle_hold(
                sink.hold_ids().first().unwrap(),
                HoldAnswer::Deny,
                &WebFetchDomains::default()
            )
            .is_some()
    );
    let dropped = decider.hold(&x, None).await;
    drop(dropped);
    clock.advance(5);
    let _fresh = decider.hold(&x, None).await;
    assert_eq!(3, sink.hold_ids().len());
}
