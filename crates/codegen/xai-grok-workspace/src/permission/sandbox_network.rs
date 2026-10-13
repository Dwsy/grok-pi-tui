//! Network hold-and-ask for the per-command sandbox: the egress proxy's [`Decider`] backed by the
//! folder's grant rows, its `allowed_web_fetch_domains` and its mode, all read through one
//! [`GrantView`] the folder's sandbox implements. Each call is decided under the mode its spawn
//! record pins: `Off` allows, `Observe` records the verdict enforce would reach, and `Enforce`
//! reads every deny source before any allow, then asks — the request is parked under one card
//! per `(call, host, port)` until [`SandboxNetworkDecider::settle_hold`] answers or the hold is
//! released (a covering row, the call's epoch ending, the proxy stopping, its deadline).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use xai_grok_egress_proxy::{Decider, Decision, DenySource, WouldBe};
use xai_grok_paths::AbsPathBuf;
use xai_grok_sandbox::WebsiteOrigin;
pub use xai_grok_sandbox::command::HoldAnswer;
use xai_grok_sandbox::command::{
    Blocked, Clock, CommandTag, Disposition, Grant, GrantDecision, GrantSubject, HostPattern,
    InformationalReason, Replay, SandboxMode, Violation,
};
use xai_grok_tools::implementations::grok_build::web_fetch::domain::normalize_domain;

use crate::permission::grants::denied_web_fetch_domain;
use crate::permission::hub_gate::grant_dir_for;
use crate::permission::state::{CachedStateStore, PermissionState, StateFileAccess};

/// Where a held network ask goes. The gate implements it over `settle_violation` and answers
/// through [`SandboxNetworkDecider::settle_hold`] with the `hold_id` carried in `replay`; an
/// informational violation (`policy_denylist`) has nothing parked under its `hold_id` and is not
/// settled back. `post` runs on the proxy connection task: enqueue and return, never wait for
/// the user. The task may be cancelled while `post` runs; the decider then takes the hold back,
/// so a card posted anyway names a `hold_id` that [`SandboxNetworkDecider::card_token`] no
/// longer knows, and is not shown.
#[async_trait]
pub trait ViolationSink: Send + Sync {
    /// Once per new hold; a coalesced duplicate for the same `(call, host, port)` is not posted
    /// again.
    async fn post(&self, call: Option<&CommandTag>, violation: Violation);
}

/// What the decider reads at every decision, owned by whoever holds the folder's grant store so
/// no second copy of the rows exists. Expiry is judged by the decider's clock.
///
/// Lock order: the synchronous methods may be called with the decider's `pending` lock held, so
/// the order is `pending` → the call table → the row snapshot, never reversed — nothing that
/// holds the call table or the rows may call back into the decider. `web_fetch_domains` is
/// awaited off every lock: it may read a file.
#[async_trait]
pub trait GrantView: Send + Sync {
    /// The folder's live `NetHost` rows, allow and deny alike (`GrantStore::live`), plus the
    /// one-shot rows stashed for `call` while it runs (a call-scoped network grant applies to
    /// the command's next connections, not only to its next spawn). Read under the decider's
    /// lock, so from memory only: no file, no wait.
    fn net_rows(&self, call: Option<&CommandTag>) -> Vec<Grant>;
    /// The mode `call` runs under, fixed once for the call: its spawn record's, or the mode the
    /// hub pinned while the spawn is still to come. `None` for no call, and for a call the
    /// table holds no mode for (no entry, or an entry with neither a spawn nor a pin). Read
    /// under the decider's lock: from memory only.
    fn call_mode(&self, call: Option<&CommandTag>) -> Option<SandboxMode>;
    /// `call`'s epoch as the table has it: it moves on when the call goes to the background
    /// (its next connections are its child's) and at its final result, so a hold parked under
    /// an earlier epoch is refused rather than answered. `None` for no call or no entry. From
    /// memory only.
    fn call_epoch(&self, call: Option<&CommandTag>) -> Option<u64>;
    /// Whether `call` has an open spawn record: a process of it may still be connecting. `false`
    /// for a call the table does not hold or whose spawn is over (its child exited, its `finish`
    /// ran): a connection that authenticated before that is refused, never parked. From memory only.
    fn call_runs(&self, call: &CommandTag) -> bool;
    /// Whether any live call — a running or background spawn, or a dispatched call not yet
    /// spawned — runs under `Enforce` right now. While one does, a request with no credential
    /// is refused whatever the folder's mode: an enforced command that dropped its token must
    /// not reach the network as the session's once the folder flipped to `observe`. From
    /// memory only.
    fn any_call_runs_enforced(&self) -> bool;
    /// The folder's mode as its owner last resolved it: a copy in memory, never a file. In the
    /// daemon the `WorkspaceSandbox`'s own reads (before each command, on each sync, after
    /// `sandbox.mode.set`) refresh it, so nothing on the proxy's connection task stats a config
    /// file. Read once per session-token decision and at every `admits_unauthenticated`.
    fn mode(&self) -> SandboxMode;
    /// The folder's remembered web-fetch domains (`permission.toml`), re-read when the file
    /// changed.
    async fn web_fetch_domains(&self) -> WebFetchDomains;
}

/// The two domain sets `permission.toml` keeps for the web-fetch tool, shared with `curl`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebFetchDomains {
    pub allowed: HashSet<String>,
    pub disallowed: HashSet<String>,
}

/// The folder's `permission.toml`, re-read when the file changed. Owned by the [`GrantView`]
/// implementor so the decider has no store of its own.
pub struct WebFetchDomainFile {
    store: CachedStateStore,
    state: PermissionState,
}

impl WebFetchDomainFile {
    /// Keyed the way the hub gate keys a session bound at `cwd` under `served_root`; with the
    /// daemon's `grok_home` this is the file the gate reads, on purpose — and read as the
    /// daemon's own file ([`StateFileAccess::DaemonOwned`]): a proxy exists only for a folder
    /// whose sandbox is on, and a `permission.toml` a command planted allows no host.
    pub async fn open(
        grok_home: PathBuf,
        cwd: &AbsPathBuf,
        served_root: &Path,
    ) -> WebFetchDomainFile {
        let grant_dir = grant_dir_for(cwd, served_root).await;
        let (store, state) = CachedStateStore::resolve_and_load_with(
            &grant_dir,
            None,
            StateFileAccess::DaemonOwned { grok_home },
        )
        .await;
        WebFetchDomainFile { store, state }
    }

    /// The sets as the file has them now.
    pub async fn current(&mut self) -> WebFetchDomains {
        if let Some(fresh) = self.store.reload_if_changed().await {
            self.state = fresh;
        }
        WebFetchDomains {
            allowed: self.state.allowed_web_fetch_domains.clone(),
            disallowed: self.state.disallowed_web_fetch_domains.clone(),
        }
    }
}

pub struct SandboxNetworkDeciderConfig {
    pub clock: Arc<dyn Clock>,
    /// How long a hold nobody is waiting on any more keeps its card's `hold_id`: the proxy's
    /// `EgressProxyOptions::hold_timeout`, so the card's deadline and the proxy's `403` agree.
    pub hold_timeout: Duration,
    /// `None` fails closed: every `Enforce` ask is a deny, as a gate without a transport is.
    pub sink: Option<Arc<dyn ViolationSink>>,
    pub view: Arc<dyn GrantView>,
}

/// The most cards one folder's decider keeps on screen at once, grantable and informational
/// together. Past it a new ask is refused and a deny-list hit raises no card.
pub const MAX_PENDING_CARDS: usize = 64;

pub struct SandboxNetworkDecider {
    clock: Arc<dyn Clock>,
    hold_timeout: Duration,
    sink: Option<Arc<dyn ViolationSink>>,
    view: Arc<dyn GrantView>,
    pending: Mutex<Books>,
}

/// The decider's books, under one lock: the cards on screen and whether the proxy they were
/// raised for is stopping. Set by [`SandboxNetworkDecider::close`] and read at every insertion
/// under the same lock, so a connection still deciding when the stop lands parks nothing.
#[derive(Default)]
struct Books {
    holds: HashMap<String, PendingHold>,
    closed: bool,
}

/// One card on the decider's books: the call it is for and that call's epoch at the park, its
/// origin, and what is parked under it.
struct PendingHold {
    call: Option<CommandTag>,
    /// [`GrantView::call_epoch`] at the park: the hold belongs to that epoch of the call, and
    /// [`SandboxNetworkDecider::release_call_holds`] for it or a later one refuses the hold.
    epoch: Option<u64>,
    host: String,
    port: u16,
    posted_unix: i64,
    parked: Parked,
    /// Cancelled when the entry goes by anything but its own answer (a row another card
    /// recorded, the call's epoch ending, the proxy stopping, its deadline): the card task
    /// still waiting on the user ends, so a late answer on that card records nothing.
    cancel: CancellationToken,
}

/// What one card holds; the two kinds never share a code path by flag.
enum Parked {
    /// A card on screen and the connections waiting on its answer. Kept while any waits, then
    /// to the card's deadline, so a retry lands under the same card instead of raising another.
    Hold { waiters: Vec<oneshot::Sender<bool>> },
    /// A `policy_denylist` card: nothing waits on it and nothing settles it. Kept to its
    /// deadline so a burst of connections raises it once.
    Informational,
}

impl Parked {
    /// Whether this is a hold a connection can join and an answer settles.
    fn is_hold(&self) -> bool {
        matches!(self, Parked::Hold { .. })
    }

    /// Everyone parked here, to be answered.
    fn into_waiters(self) -> Vec<oneshot::Sender<bool>> {
        match self {
            Parked::Hold { waiters } => waiters,
            Parked::Informational => Vec::new(),
        }
    }
}

impl PendingHold {
    fn is_for(&self, call: Option<&CommandTag>, host: &str, port: u16) -> bool {
        self.call.as_ref() == call && self.port == port && self.host.eq_ignore_ascii_case(host)
    }
}

/// A park's handle on the card it put on the books, held across the one await between the
/// insert and the card's post. It owes that card: dropped before [`Booking::published`] — the
/// proxy's hold timeout fired or its connection task was aborted while the sink was slow, or
/// the sink panicked — it takes the entry back with a deny, so nothing stays on the books that
/// no card was raised for, whoever joined it meanwhile is refused instead of waiting out a card
/// nobody sees, and the next ask for the key raises its own.
#[must_use]
struct Booking<'a> {
    decider: &'a SandboxNetworkDecider,
    hold_id: String,
    armed: bool,
}

impl<'a> Booking<'a> {
    fn new(decider: &'a SandboxNetworkDecider, hold_id: String) -> Booking<'a> {
        Booking {
            decider,
            hold_id,
            armed: true,
        }
    }

    fn id(&self) -> &str {
        &self.hold_id
    }

    /// The card is posted: the entry is the card's now, kept to its answer or its deadline.
    fn published(mut self) {
        self.armed = false;
    }
}

impl Drop for Booking<'_> {
    fn drop(&mut self) {
        if self.armed {
            tracing::info!(
                hold_id = %self.hold_id,
                "park dropped before its card was posted; the entry is taken back with a deny"
            );
            self.decider.drain(&self.hold_id, HoldAnswer::Deny);
        }
    }
}

/// The cards on screen for `(call, host, port)`, grantable and informational.
fn cards_for<'a>(
    pending: &'a HashMap<String, PendingHold>,
    call: Option<&'a CommandTag>,
    host: &'a str,
    port: u16,
) -> impl Iterator<Item = (&'a String, &'a PendingHold)> {
    pending
        .iter()
        .filter(move |(_, hold)| hold.is_for(call, host, port))
}

/// What one source of remembered decisions says about an origin.
#[derive(Clone, Copy, Default)]
struct Verdict {
    deny: bool,
    allow: bool,
}

/// The verdict `enforce` reaches from the rows and the lists: both deny sources before any
/// allow, so a deny row or the organisation's list wins over an allow row or
/// `allowed_web_fetch_domains`.
fn would_be(rows: Verdict, domains: Verdict) -> WouldBe {
    if rows.deny {
        WouldBe::Denied(DenySource::DenyRow)
    } else if domains.deny {
        WouldBe::Denied(DenySource::WebFetchDenylist)
    } else if rows.allow || domains.allow {
        WouldBe::Allowed
    } else {
        WouldBe::Asked
    }
}

/// The verdict `host` gets from the two web-fetch lists as `domains` has them.
fn domains_verdict_in(host: &str, domains: &WebFetchDomains) -> Verdict {
    Verdict {
        deny: denied_web_fetch_domain(host, &domains.disallowed).is_some(),
        allow: domains.allowed.contains(&normalize_domain(host)),
    }
}

impl SandboxNetworkDecider {
    pub fn new(config: SandboxNetworkDeciderConfig) -> SandboxNetworkDecider {
        SandboxNetworkDecider {
            clock: config.clock,
            hold_timeout: config.hold_timeout,
            sink: config.sink,
            view: config.view,
            pending: Mutex::default(),
        }
    }

    /// The folder's mode, as its owner has it now ([`GrantView::mode`]).
    pub fn mode(&self) -> SandboxMode {
        self.view.mode()
    }

    /// The mode `call`'s connection is decided under: the one its spawn record pins
    /// ([`GrantView::call_mode`]), else — a tag the table holds no mode for, or the session token
    /// — the folder's, read once per decision, failing closed to `Enforce` while the proxy admits
    /// nothing unauthenticated.
    fn mode_for(&self, call: Option<&CommandTag>) -> SandboxMode {
        match self.view.call_mode(call) {
            Some(mode) => mode,
            None if call.is_some() && !self.admits_unauthenticated() => SandboxMode::Enforce,
            None => self.mode(),
        }
    }

    /// Epoch `epoch` of `call` ended (its result is in, its child exited, or it went to the
    /// background): every hold parked for it under that epoch or an earlier one is refused, its
    /// card withdrawn; one parked under a later epoch stays. Called with no call-table guard held.
    pub fn release_call_holds(&self, call: &CommandTag, epoch: u64) {
        self.drain_holds("the call's epoch ended", |hold| {
            (hold.call.as_ref() == Some(call) && hold.epoch.is_some_and(|parked| parked <= epoch))
                .then_some(HoldAnswer::Deny)
        });
    }

    /// Every connection still parked is released with `answer` and its card entry dropped: the
    /// folder's proxy is stopping, and nothing may wait out its hold on a listener going away.
    pub fn release_all(&self, answer: HoldAnswer) {
        self.drain_holds("the folder's proxy is stopping", |_| Some(answer));
    }

    /// The proxy is stopping, for good (a restart makes a new decider): the books are closed
    /// under their lock, so a connection still between its decision and its park is refused
    /// there with no card posted and nothing put on the books, and everything already parked is
    /// released with a deny and its card withdrawn. Idempotent: the sandbox's stop and the
    /// proxy's own accept loop ([`Decider::stopping`]) both call it.
    pub fn close(&self) {
        self.pending.lock().closed = true;
        self.release_all(HoldAnswer::Deny);
    }

    /// Takes every entry `pick` answers off the books under one lock, then withdraws each with
    /// its answer off the lock: its waiters get the answer and its card's token is cancelled.
    /// The one release path; `pick` runs under `pending`, so it reads memory only.
    fn drain_holds(
        &self,
        why: &'static str,
        mut pick: impl FnMut(&PendingHold) -> Option<HoldAnswer>,
    ) {
        let drained: Vec<(PendingHold, HoldAnswer)> = {
            let mut books = self.pending.lock();
            let mut answers = Vec::new();
            // `extract_if` yields in the order the predicate accepted: `answers`' order
            let holds: Vec<PendingHold> = books
                .holds
                .extract_if(|_, hold| pick(hold).inspect(|answer| answers.push(*answer)).is_some())
                .map(|(_, hold)| hold)
                .collect();
            holds.into_iter().zip(answers).collect()
        };
        for (hold, answer) in drained {
            tracing::info!(
                host = %hold.host,
                port = hold.port,
                call = ?hold.call.as_ref().map(ToString::to_string),
                ?answer,
                "held connection released: {why}"
            );
            withdraw(hold, answer);
        }
    }

    /// Whether the card `hold_id` is still on the decider's books (parked connections, or an
    /// informational card within its deadline): a card whose hold a recorded row already released
    /// is not shown.
    pub fn is_pending(&self, hold_id: &str) -> bool {
        self.pending.lock().holds.contains_key(hold_id)
    }

    /// The token the card `hold_id`'s task waits on the user under: cancelled the moment the
    /// hold is released by anything but that card's own answer. `None` once the card is off the
    /// books (not to be shown, or already answered).
    pub fn card_token(&self, hold_id: &str) -> Option<CancellationToken> {
        self.pending
            .lock()
            .holds
            .get(hold_id)
            .map(|hold| hold.cancel.clone())
    }

    /// Answers the card `hold_id`: every request parked under it is released with `answer`,
    /// judged again first against the rows as they stand and the lists as `domains` has them
    /// (read by the caller off every lock, just before) — an `Allow` a deny row or a
    /// `disallowed_web_fetch_domains` entry now refuses is applied as a `Deny`. The answer
    /// applied comes back; `None` when the hold is unknown or already settled. Then every other
    /// parked hold is re-judged against the same rows and lists (the answer may have recorded a
    /// row that covers it — a workspace-scoped allow or deny) and released with that verdict.
    pub fn settle_hold(
        &self,
        hold_id: &str,
        answer: HoldAnswer,
        domains: &WebFetchDomains,
    ) -> Option<HoldAnswer> {
        let settled = {
            let mut books = self.pending.lock();
            // Only a hold settles: nothing is parked under an informational card (it stays to
            // its deadline)
            let hold = match books.holds.get(hold_id) {
                Some(hold) if hold.parked.is_hold() => books.holds.remove(hold_id),
                _ => None,
            };
            hold.map(|hold| {
                let applied = match self.verdict_now(&hold, domains) {
                    WouldBe::Denied(_) => HoldAnswer::Deny,
                    WouldBe::Allowed | WouldBe::Asked => answer,
                };
                (hold, applied)
            })
        };
        let applied = settled.map(|(hold, applied)| {
            release(hold, applied);
            applied
        });
        self.release_holds_now_covered(domains);
        applied
    }

    /// Holds the rows as they stand or the lists as `domains` has them now decide are released
    /// with that verdict; the rest stay parked. Called after an answer recorded a row, and
    /// after the rows or `permission.toml` were reloaded from disk.
    pub fn release_holds_now_covered(&self, domains: &WebFetchDomains) {
        self.drain_holds("a row or a list entry now covers it", |hold| {
            if !hold.parked.is_hold() {
                return None;
            }
            match self.verdict_now(hold, domains) {
                WouldBe::Denied(_) => Some(HoldAnswer::Deny),
                WouldBe::Allowed => Some(HoldAnswer::Allow),
                WouldBe::Asked => None,
            }
        });
    }

    /// What `enforce` says about `hold`'s origin from the rows as they stand and `domains`.
    fn verdict_now(&self, hold: &PendingHold, domains: &WebFetchDomains) -> WouldBe {
        would_be(
            self.rows_verdict(&hold.host, hold.port, hold.call.as_ref()),
            domains_verdict_in(&hold.host, domains),
        )
    }

    fn rows_verdict(&self, host: &str, port: u16, call: Option<&CommandTag>) -> Verdict {
        let now = self.clock.now_unix();
        let mut verdict = Verdict::default();
        for grant in self.view.net_rows(call) {
            if !grant.is_live(now) {
                continue;
            }
            let GrantSubject::NetHost {
                host: pattern,
                port: grant_port,
            } = &grant.subject
            else {
                continue;
            };
            if grant_port.is_some_and(|granted| granted != port) || !pattern.matches(host) {
                continue;
            }
            match grant.decision {
                GrantDecision::Deny => verdict.deny = true,
                GrantDecision::Allow => verdict.allow = true,
            }
        }
        verdict
    }

    /// The lists' verdict on `host`, from `permission.toml` as it is now: a file, awaited off
    /// every lock.
    async fn domains_verdict(&self, host: &str) -> Verdict {
        domains_verdict_in(host, &self.view.web_fetch_domains().await)
    }

    /// Drops the cards past their deadline that nothing waits on any more, and cancels their
    /// tokens (the card task ends). A waiter whose receiver the proxy let go of (its timeout, a
    /// hang-up) no longer counts.
    fn sweep_expired(&self, pending: &mut HashMap<String, PendingHold>, now: i64) {
        let keep_until = i64::try_from(self.hold_timeout.as_secs()).unwrap_or(i64::MAX);
        pending.retain(|_, hold| {
            let within_deadline = now < hold.posted_unix.saturating_add(keep_until);
            let kept = match &mut hold.parked {
                Parked::Informational => within_deadline,
                Parked::Hold { waiters } => {
                    waiters.retain(|waiter| !waiter.is_closed());
                    !waiters.is_empty() || within_deadline
                }
            };
            if !kept {
                hold.cancel.cancel();
            }
            kept
        });
    }

    /// The entry `hold_id`, off the books; `None` when a release already took it.
    fn take(&self, hold_id: &str) -> Option<PendingHold> {
        self.pending.lock().holds.remove(hold_id)
    }

    /// Releases what is parked under `hold_id` with `answer` and withdraws its card; nothing
    /// when a release already took it.
    fn drain(&self, hold_id: &str, answer: HoldAnswer) {
        if let Some(hold) = self.take(hold_id) {
            withdraw(hold, answer);
        }
    }

    /// Park a connection `decide` asked about: one card per `(call, host, port)` while the card's
    /// deadline runs. A row, list entry or stop that landed since the decision is judged first,
    /// under the lock; past [`MAX_PENDING_CARDS`] or without a sink the receiver is closed
    /// instead. A park dropped during the card's post takes its entry back through [`Booking`].
    async fn park(
        &self,
        host: &str,
        port: u16,
        call: Option<&CommandTag>,
    ) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        // The lists are a file: read before the lock. The rows are read under it, the lock
        // `settle_hold` releases waiters under, so a row is either seen here or releases this
        let domains = self.domains_verdict(host).await;
        let now = self.clock.now_unix();
        let booking = {
            let mut books = self.pending.lock();
            if books.closed {
                tracing::info!(
                    host,
                    port,
                    "the proxy stopped while the connection was decided; refusing"
                );
                drop(sender);
                return receiver;
            }
            let pending = &mut books.holds;
            self.sweep_expired(pending, now);
            // The epoch is read before the spawn check: a spawn ending between the two reads is
            // refused here, or parked under the epoch that ended and drained with it
            let epoch = self.view.call_epoch(call);
            match would_be(self.rows_verdict(host, port, call), domains) {
                WouldBe::Allowed => {
                    let _ = sender.send(true);
                    return receiver;
                }
                WouldBe::Denied(DenySource::DenyRow) => {
                    let _ = sender.send(false);
                    return receiver;
                }
                // Dropping the sender closes the receiver: the proxy denies
                WouldBe::Denied(DenySource::WebFetchDenylist) => {
                    drop(sender);
                    None
                }
                WouldBe::Asked => {
                    // Nothing runs for an answer to reach: refused, no card, nothing booked
                    if let Some(call) = call
                        && !self.view.call_runs(call)
                    {
                        tracing::info!(
                            host,
                            port,
                            %call,
                            "the call's spawn is over; refusing the connection it left behind"
                        );
                        drop(sender);
                        return receiver;
                    }
                    let joined = cards_for(pending, call, host, port)
                        .find(|(_, hold)| hold.parked.is_hold())
                        .map(|(id, _)| id.clone());
                    if let Some(PendingHold {
                        parked: Parked::Hold { waiters },
                        ..
                    }) = joined.and_then(|id| pending.get_mut(&id))
                    {
                        waiters.push(sender);
                        return receiver;
                    }
                    if self.sink.is_none() || pending.len() >= MAX_PENDING_CARDS {
                        tracing::warn!(
                            host,
                            port,
                            "too many sandbox network cards pending, or nobody to ask; refusing"
                        );
                        drop(sender);
                        return receiver;
                    }
                    let hold_id = uuid::Uuid::now_v7().to_string();
                    pending.insert(
                        hold_id.clone(),
                        PendingHold {
                            call: call.cloned(),
                            epoch,
                            host: host.to_owned(),
                            port,
                            posted_unix: now,
                            parked: Parked::Hold {
                                waiters: vec![sender],
                            },
                            cancel: CancellationToken::new(),
                        },
                    );
                    Some(Booking::new(self, hold_id))
                }
            }
        };
        match booking {
            None => self.inform_denylist(host, port, call).await,
            Some(booking) => {
                if let Some(sink) = &self.sink
                    && self.is_pending(booking.id())
                {
                    sink.post(call, network_violation(host, port, booking.id().to_owned()))
                        .await;
                }
                booking.published();
            }
        }
        receiver
    }

    /// A deny-list refusal under `Enforce`: the connection is refused and,
    /// once per `(call, host, port)` while the card's deadline runs and within
    /// [`MAX_PENDING_CARDS`], an informational violation is posted so the user sees why and the
    /// model is told. Nothing parks under it, and nothing is posted for a proxy that stopped.
    async fn inform_denylist(&self, host: &str, port: u16, call: Option<&CommandTag>) {
        let Some(sink) = &self.sink else {
            return;
        };
        let now = self.clock.now_unix();
        let card_id = {
            let mut books = self.pending.lock();
            if books.closed {
                return;
            }
            let pending = &mut books.holds;
            self.sweep_expired(pending, now);
            // A card of either kind on screen for the key already tells the user about it
            if cards_for(pending, call, host, port).next().is_some()
                || pending.len() >= MAX_PENDING_CARDS
            {
                return;
            }
            let card_id = uuid::Uuid::now_v7().to_string();
            pending.insert(
                card_id.clone(),
                PendingHold {
                    call: call.cloned(),
                    epoch: self.view.call_epoch(call),
                    host: host.to_owned(),
                    port,
                    posted_unix: now,
                    parked: Parked::Informational,
                    cancel: CancellationToken::new(),
                },
            );
            card_id
        };
        // A decision dropped while the sink is slow takes the card's entry back, so the next
        // connection for the key posts it instead of finding one "on screen" nobody saw
        let booking = Booking::new(self, card_id);
        sink.post(
            call,
            denylist_violation(host, port, booking.id().to_owned()),
        )
        .await;
        booking.published();
    }
}

/// The hold's own answer: its waiters are released; the card task is the caller.
fn release(hold: PendingHold, answer: HoldAnswer) {
    release_with(hold.parked.into_waiters(), answer);
}

/// Released by anything but its own answer: the waiters get `answer`, and the card task still
/// waiting on the user is cancelled, so a late answer on that card records nothing.
fn withdraw(hold: PendingHold, answer: HoldAnswer) {
    hold.cancel.cancel();
    release_with(hold.parked.into_waiters(), answer);
}

fn release_with(waiters: Vec<oneshot::Sender<bool>>, answer: HoldAnswer) {
    for waiter in waiters {
        // A receiver the proxy already dropped (timeout, hang-up) has nothing to release.
        let _ = waiter.send(answer == HoldAnswer::Allow);
    }
}

#[async_trait]
impl Decider for SandboxNetworkDecider {
    async fn decide(&self, origin: &WebsiteOrigin, call: Option<&CommandTag>) -> Decision {
        let (host, port) = (origin.hostname(), origin.port());
        // The lists are a file, awaited first; the mode and the rows are read after it with
        // no await between, so what this decision applies is what it read (`mode_for`, 4)
        let domains = self.domains_verdict(host).await;
        let mode = self.mode_for(call);
        if mode == SandboxMode::Off {
            return Decision::Allow;
        }
        let would = would_be(self.rows_verdict(host, port, call), domains);
        // Observe never blocks: the verdict is recorded, not applied
        if mode == SandboxMode::Observe {
            return Decision::AllowAndRecord { would };
        }
        match would {
            WouldBe::Allowed => Decision::Allow,
            // The user's answer, or a row a card already told the model about: silent
            WouldBe::Denied(DenySource::DenyRow) => Decision::Deny,
            WouldBe::Denied(DenySource::WebFetchDenylist) => {
                self.inform_denylist(host, port, call).await;
                Decision::Deny
            }
            // Nothing is parked yet: the proxy takes its hold budget first, then `hold`
            WouldBe::Asked => match self.sink {
                Some(_) => Decision::Ask,
                None => Decision::Deny,
            },
        }
    }

    /// Only `enforce` authenticates: under `observe` a background process whose call ended (its
    /// token revoked) is recorded as the session's, never refused — unless a live call still
    /// runs under `enforce` after the flip. Then nothing unauthenticated is admitted: the
    /// folder's mode says `observe`, but an enforced command is running, and a request that
    /// presents no token cannot be told from one of its own that dropped it (`unset
    /// HTTP_PROXY`, a `curl -x` at the port). Fail closed for every caller until that call
    /// ends: the session's own token still authenticates.
    fn admits_unauthenticated(&self) -> bool {
        self.mode() != SandboxMode::Enforce && !self.view.any_call_runs_enforced()
    }

    fn stopping(&self) {
        self.close();
    }

    async fn hold(
        &self,
        origin: &WebsiteOrigin,
        call: Option<&CommandTag>,
    ) -> oneshot::Receiver<bool> {
        self.park(origin.hostname(), origin.port(), call).await
    }
}

/// The card proposes the host itself on any port; `*` is offered only by the card's
/// "allow all network" choice, never derived from one hostname.
fn network_violation(host: &str, port: u16, hold_id: String) -> Violation {
    Violation {
        blocked: Blocked::Net {
            host: Some(host.to_owned()),
            port: Some(port),
        },
        proposed: Some(GrantSubject::NetHost {
            host: HostPattern::new(host),
            port: None,
        }),
        disposition: Disposition::Grantable,
        partial_output: None,
        replay: Replay::Resume { hold_id },
        exit_code: None,
        stderr_snippet: String::new(),
    }
}

/// The organisation's policy refused the host: informational, nothing
/// proposed, nothing parked under `card_id`. `Resume` because the proxy refused the connection
/// mid-command and the command went on — the model text says so.
fn denylist_violation(host: &str, port: u16, card_id: String) -> Violation {
    Violation {
        blocked: Blocked::Net {
            host: Some(host.to_owned()),
            port: Some(port),
        },
        proposed: None,
        disposition: Disposition::informational(InformationalReason::PolicyDenylist),
        partial_output: None,
        replay: Replay::Resume { hold_id: card_id },
        exit_code: None,
        stderr_snippet: String::new(),
    }
}

#[cfg(test)]
#[path = "sandbox_network_tests.rs"]
mod tests;
