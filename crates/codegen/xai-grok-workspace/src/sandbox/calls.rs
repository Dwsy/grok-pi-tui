//! Per-call state of the [`WorkspaceSandbox`](super::WorkspaceSandbox): one entry per [`CallId`]
//! under one lock, so a call is never half-recorded across several maps and nothing of it
//! outlives its final result. An entry also holds who dispatched the call, its fixed mode, its
//! epoch and the refusals its owner answered at the proxy; the table never holds more than
//! [`MAX_OPEN_CALLS`] entries, and every method that lets a call go returns it as [`Released`],
//! so the sandbox revokes its proxy credential and refuses its ended epoch's holds.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use xai_grok_sandbox::command::backend::OriginalArgv;
use xai_grok_sandbox::command::grants::{Grant, GrantSubject};
use xai_grok_sandbox::command::{BackendName, CallId, SandboxMode, SandboxPolicy};
use xai_tool_runtime::ToolApprovalPolicy;

use super::{WorkspaceSandbox, WorkspaceSandboxError};
use crate::permission::PermissionHookTransport;

/// Who owns a shell call while it runs: what a card raised mid-command needs and the launch hook
/// never sees — the session's tenant ceiling, its id for the grant row, the transport to its
/// owner, and the command as the tool was asked to run it. Bound by the hub's dispatch before
/// the spawn, released with the call's final result.
#[derive(Clone)]
pub struct CallOwner {
    pub session_id: String,
    pub policy: ToolApprovalPolicy,
    pub transport: Option<Arc<dyn PermissionHookTransport>>,
    /// The tool's `command` argument, for the card. The spawned argv is not it: the terminal
    /// backend runs its own bootstrap script and feeds the command through a descriptor.
    pub command: Option<String>,
}

/// What the table let go of: the calls whose proxy credential goes (a credential minted for one
/// outlives its entry, so each is handed to [`WorkspaceSandbox::revoke_released`] under the
/// table's lock) and the epochs that ended (a hold parked for one may outlive it, so each is
/// handed to [`WorkspaceSandbox::release_ended_holds`] once the lock is let go). Only the table
/// makes one.
#[must_use = "a released call's proxy credential must be revoked and its holds refused"]
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Released {
    credentials: Vec<CallId>,
    /// `None`: the table did not hold the call, so every epoch a hold names for it ended.
    epochs: Vec<(CallId, Option<u64>)>,
}

impl Released {
    /// `call`'s credential goes and its epoch `epoch` ended.
    fn one(call: &CallId, epoch: Option<u64>) -> Released {
        Released {
            credentials: vec![call.clone()],
            epochs: vec![(call.clone(), epoch)],
        }
    }

    /// Epoch `epoch` of `call` ended; its entry and credential stay.
    fn epoch(call: &CallId, epoch: u64) -> Released {
        Released {
            credentials: Vec::new(),
            epochs: vec![(call.clone(), Some(epoch))],
        }
    }

    fn extend(&mut self, other: Released) {
        self.credentials.extend(other.credentials);
        self.epochs.extend(other.epochs);
    }

    /// The calls whose credential goes.
    pub(super) fn credentials(&self) -> impl Iterator<Item = &CallId> {
        self.credentials.iter()
    }

    /// The epochs that ended, as `(call, epoch)`; `None` is every epoch.
    pub(super) fn into_epochs(self) -> impl Iterator<Item = (CallId, Option<u64>)> {
        self.epochs.into_iter()
    }
}

/// Calls the table keeps at once: the hub's owned calls, the spawns nobody finished (a tool
/// spawned outside the hub's result path) and the background children still running. Only an
/// entry none of those holds is evicted to make room; with none, a new call is refused.
pub(crate) const MAX_OPEN_CALLS: usize = 512;

/// Released ids the table remembers, so a spawn or a settlement that lands after the call's final
/// result is refused instead of re-creating the call; the oldest is forgotten past this.
const MAX_TOMBSTONES: usize = 2048;

/// Why the table made no entry for a call.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CallRefused {
    /// The table is at [`MAX_OPEN_CALLS`] and every entry is a call that may still run: the new
    /// call is refused rather than one of them evicted.
    TableFull,
    /// The call's final result is in: a spawn or a settlement for it is late.
    Released,
}

/// Refusal notes one call carries into its result; past this the rest are counted in one line,
/// so a command that hits many refused hosts cannot grow the model's text without a bound.
pub(crate) const MAX_NET_DENIAL_NOTES: usize = 8;

/// What `prepare` kept about one spawn, consumed by `finish`.
pub(super) struct CallRecord {
    pub policy: SandboxPolicy,
    /// The call's mode: pinned at dispatch or taken from the folder at the first `prepare`, and
    /// what the spawn, its replay and every connection of the call run under.
    pub mode: SandboxMode,
    pub backend: Option<BackendName>,
    /// The wrapper was applied (`enforce` with a backend): only then is a denial decodable.
    pub sandboxed: bool,
    pub original: OriginalArgv,
    /// Proposal bases the command's environment named (`$PYTHONUSERBASE`, `$XDG_DATA_HOME`).
    pub extra_bases: Vec<PathBuf>,
    /// Second spawn of the same call id after a grant: a violation now is final. The subject the
    /// user allowed, so the final text can name it.
    pub replayed_under: Option<GrantSubject>,
    /// The call-scoped grants this spawn inherited (its policy was built with them): its
    /// connections are decided under them too.
    pub inherited_once: Vec<Grant>,
}

impl CallRecord {
    /// What this spawn was prepared under, for a retry of the same spawn.
    fn settling(&self) -> Settling {
        Settling {
            once: self.inherited_once.clone(),
            replayed_under: self.replayed_under.clone(),
            mode: Some(self.mode),
        }
    }
}

/// What the call's next spawn attempt runs under: left in the entry for it, then held by its
/// spawn record until `finish`.
#[derive(Clone, Default)]
pub(super) struct Settling {
    /// Call-scoped grants: in effect for the running spawn's connections as well.
    pub once: Vec<Grant>,
    /// That spawn is the replay, under this grant subject: a violation from it is final.
    pub replayed_under: Option<GrantSubject>,
    /// The call's mode, fixed at the hub's dispatch or by the spawn before: the next spawn runs
    /// under it whatever the folder's mode is by then. `None`: a first spawn outside the hub,
    /// which takes the folder's.
    pub mode: Option<SandboxMode>,
}

/// Everything the table knows about one call.
#[derive(Default)]
struct CallEntry {
    /// Who dispatched the call, bound by the hub before the spawn, released with the final result.
    owner: Option<CallOwner>,
    /// The spawn `prepare` recorded and `finish` has not taken back: its child may be running.
    spawn: Option<Box<CallRecord>>,
    /// What the next spawn inherits: `Some` from a pin, a call grant or a violation `finish`
    /// decoded, until that spawn's record holds it.
    next: Option<Settling>,
    /// The model text of each held request the owner refused while the call ran (distinct, at
    /// most [`MAX_NET_DENIAL_NOTES`]), appended to the call's result (the command itself only saw
    /// the proxy's 403); the refusals past the cap are only counted.
    net_denials: Vec<String>,
    net_denials_dropped: usize,
    /// The call's result was a background start and its child still runs: the hub session it
    /// ran for, whose end drops the entry.
    detached: Option<String>,
    /// The call's epoch: which run of the call a hold, a card or a call grant belongs to. Unique
    /// and increasing across the table; moves on at `detach` and at an owned child's `exited`.
    epoch: u64,
}

impl CallEntry {
    fn is_empty(&self) -> bool {
        self.owner.is_none()
            && self.spawn.is_none()
            && self.next.is_none()
            && self.net_denials.is_empty()
            && self.net_denials_dropped == 0
            && self.detached.is_none()
    }

    /// The call's mode is pinned and no spawn took it yet.
    fn pin_waiting(&self) -> bool {
        self.next.as_ref().is_some_and(|next| next.mode.is_some())
    }

    /// Nothing of the call can still run: no owner holds its result, no spawn is open (so no
    /// child, backgrounded or not, and no proxy credential for it), and no pin waits for its
    /// spawn.
    fn evictable(&self) -> bool {
        self.owner.is_none()
            && self.spawn.is_none()
            && self.detached.is_none()
            && !self.pin_waiting()
    }

    /// The mode the call runs under: its open spawn's, else the one pinned for its next spawn.
    fn mode(&self) -> Option<SandboxMode> {
        match &self.spawn {
            Some(spawn) => Some(spawn.mode),
            None => self.next.as_ref().and_then(|next| next.mode),
        }
    }

    /// The refusals the call carries, for its result: the distinct notes, then one line for
    /// the refusals past the cap. The entry is left with none.
    fn take_denials(&mut self) -> Vec<String> {
        let mut denials = std::mem::take(&mut self.net_denials);
        let dropped = std::mem::take(&mut self.net_denials_dropped);
        if dropped > 0 {
            denials.push(format!(
                "{dropped} more connections were refused while the command ran"
            ));
        }
        denials
    }
}

#[derive(Default)]
pub(super) struct CallTable {
    calls: HashMap<CallId, CallEntry>,
    next_epoch: u64,
    /// Ids released at their final result, oldest first.
    tombstones: VecDeque<CallId>,
    /// The dispatch mode of each call the table had no room to pin: the least its spawn runs
    /// under. Dropped with the call's release, or at `detach` (the spawn record holds the mode).
    floors: HashMap<CallId, SandboxMode>,
}

impl CallTable {
    /// How many spawns are prepared and not yet finished.
    pub fn prepared(&self) -> usize {
        self.calls
            .values()
            .filter(|entry| entry.spawn.is_some())
            .count()
    }

    /// How many calls the table holds anything for (tests).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.calls.len()
    }

    /// The record of a running (prepared) call, for a card raised mid-command.
    pub fn prepared_record(&self, call: &CallId) -> Option<&CallRecord> {
        self.calls.get(call)?.spawn.as_deref()
    }

    /// The hub dispatched `call` for `owner`: its entry stays, never evicted, until `release` or
    /// `detach`. A tombstone for the id is cleared: this is a new call under it.
    pub fn bind_owner(&mut self, call: &CallId, owner: CallOwner) -> Result<Released, CallRefused> {
        let (entry, evicted) = self.dispatched(call)?;
        entry.owner = Some(owner);
        Ok(evicted)
    }

    /// `bind_owner` for a call the hub pinned: only while its entry still holds the pin. `false`
    /// creates nothing, and the call must not spawn: without the pin it would follow the folder.
    pub fn bind_pinned_owner(&mut self, call: &CallId, owner: CallOwner) -> bool {
        match self.calls.get_mut(call) {
            Some(entry) if entry.pin_waiting() => {
                entry.owner = Some(owner);
                true
            }
            _ => false,
        }
    }

    /// The table half of [`WorkspaceSandbox::release_call`]: the id is tombstoned, and the
    /// refusals it carried come back in the same step.
    pub fn release(&mut self, call: &CallId) -> (Vec<String>, Released) {
        let notes = self.drain_denials(call);
        self.floors.remove(call);
        let released = self.remove(call);
        if !self.tombstones.contains(call) {
            if self.tombstones.len() >= MAX_TOMBSTONES {
                self.tombstones.pop_front();
            }
            self.tombstones.push_back(call.clone());
        }
        (notes, released)
    }

    /// The entry goes without a tombstone: an eviction takes a stray nobody will spawn again.
    fn remove(&mut self, call: &CallId) -> Released {
        let epoch = self.calls.remove(call).map(|entry| entry.epoch);
        Released::one(call, epoch)
    }

    pub fn owner_of(&self, call: &CallId) -> Option<CallOwner> {
        self.calls.get(call).and_then(|entry| entry.owner.clone())
    }

    /// The hub session `call` runs for: its owner's, or after a background start the one its
    /// child still runs for. `None` for a call no session owns (a stray spawn): it runs under the
    /// rows every session shares, never under one session's own.
    pub fn session_of(&self, call: &CallId) -> Option<String> {
        let entry = self.calls.get(call)?;
        entry
            .owner
            .as_ref()
            .map(|owner| owner.session_id.clone())
            .or_else(|| entry.detached.clone())
    }

    /// `call`'s epoch as the table has it; `None` for a call it does not hold.
    pub fn epoch_of(&self, call: &CallId) -> Option<u64> {
        self.calls.get(call).map(|entry| entry.epoch)
    }

    /// A refusal to tell the call's owner with its result. A late answer for a call whose result
    /// is already in has nobody to tell and is dropped, never re-creating the call.
    pub fn push_denial(&mut self, call: &CallId, model_text: String) {
        if let Some(entry) = self.calls.get_mut(call) {
            if entry.net_denials.contains(&model_text) {
                return;
            }
            if entry.net_denials.len() < MAX_NET_DENIAL_NOTES {
                entry.net_denials.push(model_text);
            } else {
                entry.net_denials_dropped += 1;
            }
        }
    }

    /// The refusals `call` carries, consumed; an entry with nothing else left goes with them.
    pub fn take_denials(&mut self, call: &CallId) -> Vec<String> {
        let denials = self.drain_denials(call);
        self.drop_if_empty(call);
        denials
    }

    /// [`CallEntry::take_denials`] for `call`; none for a call the table does not hold.
    fn drain_denials(&mut self, call: &CallId) -> Vec<String> {
        self.calls
            .get_mut(call)
            .map(CallEntry::take_denials)
            .unwrap_or_default()
    }

    /// The hub dispatched `call` under `mode`, the folder's mode as it read it to choose the
    /// call's path: the call's spawns and connections run under it whatever the folder's mode
    /// is by then. A tombstone for the id is cleared: this is a new call under it. A call that
    /// already holds a mode keeps it (the hub never pins a live call twice; the table does not
    /// rely on that).
    pub fn pin_mode(&mut self, call: &CallId, mode: SandboxMode) -> Result<Released, CallRefused> {
        let (entry, evicted) = self.dispatched(call)?;
        match entry.mode() {
            None => entry.next.get_or_insert_default().mode = Some(mode),
            Some(held) if held != mode => {
                tracing::warn!(%call, ?held, ?mode, "the call already runs under a mode; the pin keeps it");
            }
            Some(_) => {}
        }
        Ok(evicted)
    }

    /// The mode `call` runs under whatever the folder's is by now: its open spawn record's, else
    /// the one pinned for its next spawn. `None` for a call the table holds no mode for, whose
    /// first spawn takes the folder's.
    pub fn held_mode(&self, call: &CallId) -> Option<SandboxMode> {
        self.calls.get(call)?.mode()
    }

    /// `pin_mode` had no room for `call`, dispatched under `mode`: its spawn runs under the
    /// folder's mode then or `mode`, whichever is stronger. `false` past [`MAX_OPEN_CALLS`]
    /// floors (fail closed: the call must not run); a floor the call already has only tightens.
    pub fn floor_mode(&mut self, call: &CallId, mode: SandboxMode) -> bool {
        if !self.floors.contains_key(call) && self.floors.len() >= MAX_OPEN_CALLS {
            tracing::warn!(
                %call,
                "as many unpinned sandbox calls as the table holds; refusing the call"
            );
            return false;
        }
        let floor = self.floors.entry(call.clone()).or_insert(mode);
        *floor = (*floor).max(mode);
        true
    }

    /// The least the spawn of `call` runs under, for a call the table holds no mode for.
    pub fn floor_of(&self, call: &CallId) -> Option<SandboxMode> {
        self.floors.get(call).copied()
    }

    /// The floors of the calls that hold no mode: the least their spawns run under. A call that
    /// holds a mode runs under it, no weaker than its floor, and counts once by it.
    fn floors_in_force(&self) -> impl Iterator<Item = SandboxMode> + '_ {
        self.floors
            .iter()
            .filter(|(call, _)| self.held_mode(call).is_none())
            .map(|(_, floor)| *floor)
    }

    /// Whether any live call — a running or background spawn, a pinned call not yet spawned, or
    /// an unpinned one floored at `enforce` — runs under `enforce` right now: while one does, the
    /// proxy authenticates every request whatever the folder's mode.
    pub fn any_runs_enforced(&self) -> bool {
        self.calls
            .values()
            .any(|entry| entry.mode() == Some(SandboxMode::Enforce))
            || self
                .floors_in_force()
                .any(|floor| floor == SandboxMode::Enforce)
    }

    /// How many live calls run under a mode other than `mode`: what a mode change leaves as it
    /// is, for its reply. A floored call runs under the stronger of its floor and `mode`, so it
    /// counts only when its floor is the stronger.
    pub fn running_under_another_mode(&self, mode: SandboxMode) -> usize {
        let held = self
            .calls
            .values()
            .filter(|entry| entry.mode().is_some_and(|held| held != mode))
            .count();
        held + self.floors_in_force().filter(|floor| *floor > mode).count()
    }

    /// What the next spawn of `call` runs under, left where it is: an open spawn record's (a
    /// retry of a try that got no child), else what was left for the next spawn. A call with
    /// nothing pending is a first run with no call-scoped grant.
    pub fn attempt_settling(&self, call: &CallId) -> Settling {
        let Some(entry) = self.calls.get(call) else {
            return Settling::default();
        };
        match &entry.spawn {
            Some(spawn) => spawn.settling(),
            None => entry.next.clone().unwrap_or_default(),
        }
    }

    /// Keep `record` for `finish`. A first try's record now holds what was left for the spawn, so
    /// that leaves the entry — only what the record copied: a call grant or a replay mark that
    /// landed since `attempt_settling` read them (the policy build runs in between) stays, for
    /// the spawn's connections and its next spawn. A retry's record replaces the open one, which
    /// already held it. Refused for a released call: the spawn is late.
    pub fn remember(&mut self, call: &CallId, record: CallRecord) -> Result<Released, CallRefused> {
        let (entry, evicted) = self.entry(call)?;
        if entry.spawn.is_none()
            && let Some(next) = entry.next.take()
        {
            let left = Settling {
                once: next
                    .once
                    .into_iter()
                    .filter(|grant| {
                        !record
                            .inherited_once
                            .iter()
                            .any(|copied| copied.id == grant.id)
                    })
                    .collect(),
                replayed_under: next
                    .replayed_under
                    .filter(|mark| record.replayed_under.as_ref() != Some(mark)),
                mode: next.mode.filter(|pinned| *pinned != record.mode),
            };
            if !left.once.is_empty() || left.replayed_under.is_some() || left.mode.is_some() {
                entry.next = Some(left);
            }
        }
        entry.spawn = Some(Box::new(record));
        Ok(evicted)
    }

    /// The spawn of `call` is over: the record `prepare` kept for it, if one is open, and the
    /// call released, record or not, so its credential stops working (a replay mints its own)
    /// and what its connections still hold is refused. What was left for the next spawn stays
    /// until `finish` opens or closes the settlement.
    pub fn take_prepared(&mut self, call: &CallId) -> (Option<Box<CallRecord>>, Released) {
        let entry = self.calls.get_mut(call);
        let epoch = entry.as_ref().map(|entry| entry.epoch);
        let record = entry.and_then(|entry| entry.spawn.take());
        (record, Released::one(call, epoch))
    }

    /// `finish` decoded a violation from `call`'s spawn: the gate settles it into what the next
    /// spawn (the replay) inherits, under `mode`, the spawn's own. The mode is kept for a call
    /// someone owns: nothing replays a stray, and its leftover stays one nothing of which runs,
    /// so it can make room. Refused for a released call.
    pub fn open_settlement(
        &mut self,
        call: &CallId,
        mode: SandboxMode,
    ) -> Result<Released, CallRefused> {
        let (entry, evicted) = self.entry(call)?;
        let owned = entry.owner.is_some();
        let next = entry.next.get_or_insert_default();
        if owned {
            next.mode.get_or_insert(mode);
        }
        Ok(evicted)
    }

    /// `finish` found nothing to settle: no spawn of `call` follows, so nothing carries over.
    pub fn close_settlement(&mut self, call: &CallId) {
        if let Some(entry) = self.calls.get_mut(call) {
            entry.next = None;
        }
        self.drop_if_empty(call);
    }

    /// A call-scoped grant for `call`, answered on a card raised under `epoch` (`None`: the
    /// call as the table has it now): in effect for its running spawn's connections and
    /// inherited by its next spawn. `false` when the call is gone or its epoch moved on since
    /// the card: the grant is kept nowhere.
    pub fn stash_once(&mut self, call: &CallId, epoch: Option<u64>, grant: Grant) -> bool {
        match self.calls.get_mut(call) {
            Some(entry) if epoch.is_none_or(|epoch| epoch == entry.epoch) => {
                entry.next.get_or_insert_default().once.push(grant);
                true
            }
            Some(_) => {
                tracing::info!(%call, grant = %grant.id, "call grant from before the call's epoch moved on dropped");
                false
            }
            None => {
                tracing::info!(%call, grant = %grant.id, "call grant for a finished call dropped");
                false
            }
        }
    }

    /// The next spawn of `call` is the replay under `granted` (a violation then is final).
    pub fn mark_replay(&mut self, call: &CallId, granted: GrantSubject) {
        match self.calls.get_mut(call) {
            Some(entry) => entry.next.get_or_insert_default().replayed_under = Some(granted),
            None => tracing::info!(%call, "replay mark for a finished call dropped"),
        }
    }

    /// The call-scoped rows in effect for `call`: the ones its running spawn inherited and the
    /// ones given since.
    pub fn once_rows(&self, call: &CallId) -> Vec<Grant> {
        let Some(entry) = self.calls.get(call) else {
            return Vec::new();
        };
        let inherited = entry.spawn.iter().flat_map(|spawn| &spawn.inherited_once);
        let given = entry.next.iter().flat_map(|next| &next.once);
        inherited.chain(given).cloned().collect()
    }

    /// The table half of [`WorkspaceSandbox::detach_call`]: the entry stays for the child, its
    /// call-scoped rows go, its epoch moves on; a call with no open spawn is released instead.
    pub fn detach(&mut self, call: &CallId, session: &str) -> (Vec<String>, Released) {
        let Some(entry) = self.calls.get_mut(call) else {
            return (Vec::new(), Released::default());
        };
        let Some(spawn) = entry.spawn.as_mut() else {
            return self.release(call);
        };
        spawn.inherited_once.clear();
        if let Some(next) = entry.next.as_mut() {
            next.once.clear();
        }
        entry.owner = None;
        entry.detached = Some(session.to_owned());
        let ended = entry.epoch;
        entry.epoch = self.next_epoch;
        self.next_epoch += 1;
        let denials = entry.take_denials();
        self.floors.remove(call);
        (denials, Released::epoch(call, ended))
    }

    /// A backgrounded child of `call` exited: its spawn record, its call-scoped rows, its credential
    /// and the epoch its holds were parked under go, hub result or not. A call nobody owns is dropped
    /// and released (a refusal since its start has no result to go to); an owned one keeps its entry.
    pub fn exited(&mut self, call: &CallId) -> Released {
        let Some(entry) = self.calls.get_mut(call) else {
            return Released::default();
        };
        if entry.owner.is_some() {
            // A repeat for a spawn already gone ends nothing twice
            if entry.spawn.take().is_none() {
                return Released::default();
            }
            if let Some(next) = entry.next.as_mut() {
                next.once.clear();
            }
            let ended = entry.epoch;
            entry.epoch = self.next_epoch;
            self.next_epoch += 1;
            return Released::one(call, Some(ended));
        }
        self.release(call).1
    }

    /// Hub session `session` ended: each call it left running in the background is dropped and
    /// released (its credential goes). Their results are long in: no refusal is kept.
    pub fn end_session(&mut self, session: &str) -> Released {
        let ended: Vec<CallId> = self
            .calls
            .iter()
            .filter(|(_, entry)| entry.detached.as_deref() == Some(session))
            .map(|(call, _)| call.clone())
            .collect();
        let mut released = Released::default();
        for call in &ended {
            released.extend(self.release(call).1);
        }
        released
    }

    /// The entry for `call` at the hub's dispatch: a tombstone for the id is cleared first, so a
    /// new call under a released id starts afresh with its own epoch.
    fn dispatched(&mut self, call: &CallId) -> Result<(&mut CallEntry, Released), CallRefused> {
        self.tombstones.retain(|released| released != call);
        self.entry(call)
    }

    /// The entry for `call`, created if absent; the one eviction rule runs here, whoever creates
    /// it. A released id is refused: what creates it late (a spawn, a settlement) must not
    /// re-create the call. At [`MAX_OPEN_CALLS`] the oldest entry nothing of which may still run
    /// is dropped and released. With none such the call is refused (fail closed): a call that
    /// may be running is never dropped.
    fn entry(&mut self, call: &CallId) -> Result<(&mut CallEntry, Released), CallRefused> {
        let mut evicted = Released::default();
        if !self.calls.contains_key(call) {
            if self.tombstones.contains(call) {
                tracing::info!(%call, "sandbox call already released; refusing to re-create it");
                return Err(CallRefused::Released);
            }
            if self.calls.len() >= MAX_OPEN_CALLS {
                let Some(oldest) = self
                    .calls
                    .iter()
                    .filter(|(_, entry)| entry.evictable())
                    .min_by_key(|(_, entry)| entry.epoch)
                    .map(|(id, _)| id.clone())
                else {
                    tracing::warn!(
                        %call,
                        "sandbox call table full of calls that may still run; refusing the call"
                    );
                    return Err(CallRefused::TableFull);
                };
                tracing::warn!(
                    call = %oldest,
                    "sandbox call table full; dropping the oldest call nothing of which still runs"
                );
                evicted = self.remove(&oldest);
            }
        }
        let next_epoch = &mut self.next_epoch;
        let entry = self.calls.entry(call.clone()).or_insert_with(|| {
            let epoch = *next_epoch;
            *next_epoch += 1;
            CallEntry {
                epoch,
                ..CallEntry::default()
            }
        });
        Ok((entry, evicted))
    }

    /// An empty entry holds no spawn, so no credential: nothing is released with it.
    fn drop_if_empty(&mut self, call: &CallId) {
        if self.calls.get(call).is_some_and(CallEntry::is_empty) {
            self.calls.remove(call);
        }
    }
}

impl WorkspaceSandbox {
    /// Bind the session that dispatched `call`, so a connection held while it runs can be
    /// asked about.
    ///
    /// # Errors
    /// [`WorkspaceSandboxError::CallTableFull`]: every call the table holds may still run. The
    /// call runs unbound, and a spawn of it is refused at `prepare`.
    pub fn bind_call(&self, call: &CallId, owner: CallOwner) -> Result<(), WorkspaceSandboxError> {
        let evicted = {
            let mut calls = self.calls.lock();
            let evicted = calls.bind_owner(call, owner)?;
            self.revoke_released(&evicted);
            evicted
        };
        self.release_ended_holds(evicted);
        Ok(())
    }

    /// [`WorkspaceSandbox::bind_call`] for a call the hub let past its pre-run approval on the
    /// enforce pin: `false` when the sandbox no longer holds that pin, and the call must not run.
    #[must_use]
    pub fn bind_pinned_call(&self, call: &CallId, owner: CallOwner) -> bool {
        self.calls.lock().bind_pinned_owner(call, owner)
    }

    /// The one way a call ends: its final result is in (a kept denial or output, a tool error)
    /// or its stream ended without one. Nothing more is asked for it; its owner, its open spawn
    /// record and its pending settlement go, its proxy credential is revoked, and every hold
    /// still parked for it is refused and its card withdrawn. What comes back is every held
    /// request the owner refused while the call ran, as model text, drained in the same step
    /// so a result of any shape carries them. A background start's entry is kept instead
    /// ([`WorkspaceSandbox::detach_call`]). The last call running under `enforce` ending while
    /// the folder is not takes the proxy kept for it along
    /// ([`WorkspaceSandbox::resync_unless_enforced`]).
    pub fn release_call(&self, call: &CallId) -> Vec<String> {
        let (notes, released, enforced) = {
            let mut calls = self.calls.lock();
            let (notes, released) = calls.release(call);
            self.revoke_released(&released);
            (notes, released, calls.any_runs_enforced())
        };
        self.release_ended_holds(released);
        self.resync_unless_enforced(enforced);
        notes
    }

    /// The call's stream ended with its command still running (a background start for hub
    /// session `session_id`): nothing more is asked for the call, so its owner goes, and with
    /// it the call-scoped rows — "allow once, for this command" was answered for a command
    /// nobody can ask about any more, and a process that picked the token up from the
    /// environment must not inherit them — and every hold still parked for the command is
    /// refused and its card withdrawn (the call's epoch moves on). The process it left behind
    /// keeps the call's spawn record (its mode included) and keeps `HTTP_PROXY` pointed at the
    /// call's credential, so a host the folder or the session allows still answers it — until
    /// the child exits ([`SandboxLaunch::exited`](xai_grok_tools::sandbox_launch::SandboxLaunch::exited)),
    /// the session ends or the folder's proxy stops; then the credential is revoked with the
    /// record, so a grandchild the job daemonized authenticates no longer. The table never evicts
    /// that entry to make room. A call with no open spawn left is released now. A connection
    /// nobody allowed is refused without a card: there is no stream to ask on. What comes back
    /// is what [`WorkspaceSandbox::release_call`] returns: the refusals answered while the call
    /// ran, for the background start's result.
    pub fn detach_call(&self, call: &CallId, session_id: &str) -> Vec<String> {
        let (notes, released, enforced) = {
            let mut calls = self.calls.lock();
            let (notes, released) = calls.detach(call, session_id);
            self.revoke_released(&released);
            (notes, released, calls.any_runs_enforced())
        };
        self.release_ended_holds(released);
        self.resync_unless_enforced(enforced);
        notes
    }

    /// Revoke the proxy credential of each call the table let go of, while its lock is still
    /// held: the only caller of the proxy's revoke. The holds are refused afterwards, off the
    /// lock ([`WorkspaceSandbox::release_ended_holds`]).
    pub(super) fn revoke_released(&self, released: &Released) {
        for call in released.credentials() {
            self.revoke_call_credential(call);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use xai_grok_sandbox::command::backend::OriginalArgv;
    use xai_grok_sandbox::command::grants::{
        Expiry, Grant, GrantDecision, GrantId, GrantScope, GrantSubject, HostPattern,
    };
    use xai_grok_sandbox::command::policy::{EnvPolicy, NetworkPolicy, ReadPolicy};
    use xai_grok_sandbox::command::{CallId, SandboxMode, SandboxPolicy};
    use xai_tool_runtime::ToolApprovalPolicy;

    use super::{
        CallOwner, CallRecord, CallRefused, CallTable, MAX_NET_DENIAL_NOTES, MAX_OPEN_CALLS,
        MAX_TOMBSTONES, Released,
    };

    fn owner() -> CallOwner {
        CallOwner {
            session_id: "s-1".to_owned(),
            policy: ToolApprovalPolicy::GrantsAllowed,
            transport: None,
            command: None,
        }
    }

    fn record(once: Vec<Grant>) -> CallRecord {
        CallRecord {
            policy: SandboxPolicy {
                read: ReadPolicy::AllExcept { deny: Vec::new() },
                write_roots: Vec::new(),
                network: NetworkPolicy::Off,
                env: EnvPolicy::default(),
                protected: Vec::new(),
                build_cache_trees: Vec::new(),
                unread_git_metadata: Vec::new(),
            },
            mode: SandboxMode::Enforce,
            backend: None,
            sandboxed: true,
            original: OriginalArgv {
                program: PathBuf::from("/bin/true"),
                args: Vec::new(),
                cwd: PathBuf::from("/"),
            },
            extra_bases: Vec::new(),
            replayed_under: None,
            inherited_once: once,
        }
    }

    fn once_grant(host: &str) -> Grant {
        Grant {
            id: GrantId::new(format!("g-{host}")),
            subject: GrantSubject::NetHost {
                host: HostPattern::new(host),
                port: None,
            },
            scope: GrantScope::Call,
            expires: Expiry::Never,
            decision: GrantDecision::Allow,
            granted_at: 1_700_000_000,
            granted_by: "hub:test".to_owned(),
            via: None,
        }
    }

    #[test]
    fn an_entry_goes_when_nothing_is_left_and_unbind_takes_it_whole() {
        let mut table = CallTable::default();
        let call = CallId::tool("c1");
        table.push_denial(&call, "refused example.com".to_owned());
        assert_eq!(
            0,
            table.len(),
            "a late refusal never re-creates a finished call"
        );
        table.stash_once(&call, None, once_grant("a.example"));
        assert_eq!(0, table.len(), "nor does a late call grant");
        assert_eq!(
            Released::default(),
            table.pin_mode(&call, SandboxMode::Enforce).unwrap()
        );
        assert_eq!(
            Some(SandboxMode::Enforce),
            table.attempt_settling(&call).mode
        );
        assert_eq!(
            Released::default(),
            table.remember(&call, record(Vec::new())).unwrap()
        );
        let (spawn, released) = table.take_prepared(&call);
        assert!(spawn.is_some());
        assert_eq!(
            Released::one(&call, Some(0)),
            released,
            "its credential goes with it"
        );
        table.close_settlement(&call);
        assert_eq!(0, table.len(), "a settled entry does not linger");

        assert_eq!(
            Released::default(),
            table.bind_owner(&call, owner()).unwrap()
        );
        assert_eq!(
            Released::default(),
            table.remember(&call, record(Vec::new())).unwrap()
        );
        table.push_denial(&call, "refused b.example".to_owned());
        assert_eq!(1, table.len());
        assert_eq!(1, table.prepared());
        assert_eq!(
            (
                vec!["refused b.example".to_owned()],
                Released::one(&call, Some(1))
            ),
            table.release(&call),
            "the refusals come back with the release, for the result"
        );
        assert_eq!(0, table.len(), "owner, open spawn and refusals go together");
        assert_eq!(0, table.prepared());
        assert!(table.take_denials(&call).is_empty());
    }

    /// A call's refusal notes are distinct and capped: past [`MAX_NET_DENIAL_NOTES`] the rest are
    /// counted in one closing line, so the model's text stays bounded.
    #[test]
    fn refusal_notes_are_distinct_and_capped() {
        let mut table = CallTable::default();
        let call = CallId::tool("c1");
        assert_eq!(
            Released::default(),
            table.bind_owner(&call, owner()).unwrap()
        );
        table.push_denial(&call, "refused a.example".to_owned());
        table.push_denial(&call, "refused a.example".to_owned());
        for n in 0..MAX_NET_DENIAL_NOTES + 4 {
            table.push_denial(&call, format!("refused h{n}.example"));
        }
        let notes = table.take_denials(&call);
        assert_eq!(MAX_NET_DENIAL_NOTES + 1, notes.len(), "{notes:?}");
        assert_eq!(
            1,
            notes.iter().filter(|n| *n == "refused a.example").count()
        );
        assert_eq!(
            Some("5 more connections were refused while the command ran"),
            notes.last().map(String::as_str),
            "the note prefix is added once, where the notes are rendered"
        );
        assert!(table.take_denials(&call).is_empty());
    }

    #[test]
    fn once_grants_given_mid_run_survive_finish_for_the_replay_only() {
        let mut table = CallTable::default();
        let call = CallId::tool("c1");
        assert_eq!(
            Released::default(),
            table.remember(&call, record(Vec::new())).unwrap()
        );
        table.stash_once(&call, None, once_grant("a.example"));
        assert_eq!(1, table.once_rows(&call).len(), "the running spawn sees it");

        assert!(table.take_prepared(&call).0.is_some(), "prepared");
        assert_eq!(
            Released::default(),
            table.open_settlement(&call, SandboxMode::Enforce).unwrap()
        );
        assert_eq!(1, table.once_rows(&call).len(), "stashed for the replay");
        assert!(
            table.take_prepared(&call).0.is_none(),
            "finish consumed the record"
        );

        let settling = table.attempt_settling(&call);
        assert_eq!(1, settling.once.len());
        assert_eq!(
            Released::default(),
            table.remember(&call, record(settling.once)).unwrap()
        );
        assert_eq!(1, table.once_rows(&call).len(), "the replay holds it, once");
        assert!(table.take_prepared(&call).0.is_some());
        table.close_settlement(&call);
        assert_eq!(0, table.len(), "the replay's finish drained the settlement");
        assert!(
            table.attempt_settling(&call).once.is_empty(),
            "a later spawn is a first run"
        );
    }

    /// `prepare` reads what the spawn inherits under one lock and keeps its record under another,
    /// with the policy build in between: a call grant or a replay mark that lands in that window
    /// (a card answered while the replay prepares) is not lost — the spawn's connections see the
    /// grant, and both carry to the next spawn — while what the record did copy leaves the entry
    /// once, never doubled.
    #[test]
    fn what_lands_while_prepare_runs_is_kept_for_the_spawn_and_its_next_spawn() {
        let mut table = CallTable::default();
        let call = CallId::tool("c1");
        assert_eq!(
            Released::default(),
            table.bind_owner(&call, owner()).unwrap()
        );
        assert!(table.stash_once(&call, None, once_grant("first.example")));
        let settling = table.attempt_settling(&call);
        assert_eq!(1, settling.once.len());

        // The window: a card answered "allow for this call" and a replay mark land
        assert!(table.stash_once(&call, None, once_grant("late.example")));
        let granted = GrantSubject::NetHost {
            host: HostPattern::new("marked.example"),
            port: None,
        };
        table.mark_replay(&call, granted.clone());
        assert_eq!(
            Released::default(),
            table.remember(&call, record(settling.once)).unwrap()
        );

        let rows: Vec<String> = table
            .once_rows(&call)
            .iter()
            .map(|grant| grant.id.to_string())
            .collect();
        assert_eq!(
            vec!["g-first.example".to_owned(), "g-late.example".to_owned()],
            rows,
            "the running spawn sees both, the copied one once"
        );
        assert!(table.take_prepared(&call).0.is_some());
        assert_eq!(
            Released::default(),
            table.open_settlement(&call, SandboxMode::Enforce).unwrap()
        );
        let next = table.attempt_settling(&call);
        assert_eq!(
            vec!["g-late.example".to_owned()],
            next.once
                .iter()
                .map(|grant| grant.id.to_string())
                .collect::<Vec<_>>(),
            "the late grant carries to the next spawn"
        );
        assert_eq!(Some(granted), next.replayed_under, "so does the mark");
    }

    /// At the cap only the oldest entry nothing of which may still run goes, dropped whole and
    /// released; an owned call, a spawn nobody finished, a waiting pin and a background child
    /// never do, and with none such left a new call is refused. A late answer creates none.
    #[test]
    fn the_table_evicts_only_a_call_that_cannot_still_run_and_refuses_past_the_cap() {
        let mut table = CallTable::default();
        let leftover = CallId::tool("c0");
        assert_eq!(
            Released::default(),
            table
                .open_settlement(&leftover, SandboxMode::Enforce)
                .unwrap()
        );
        assert!(table.stash_once(&leftover, None, once_grant("a.example")));
        let stray = CallId::tool("stray");
        assert_eq!(
            Released::default(),
            table.remember(&stray, record(Vec::new())).unwrap()
        );
        let background = CallId::tool("background");
        assert_eq!(
            Released::default(),
            table.bind_owner(&background, owner()).unwrap()
        );
        assert_eq!(
            Released::default(),
            table.remember(&background, record(Vec::new())).unwrap()
        );
        assert_eq!(
            Released::epoch(&background, 2),
            table.detach(&background, "s-1").1,
            "kept for its child; only the epoch before the start ended"
        );
        let pinned = CallId::tool("pinned");
        assert_eq!(
            Released::default(),
            table.pin_mode(&pinned, SandboxMode::Enforce).unwrap()
        );
        let first_owned = CallId::tool("c4");
        for i in 4..MAX_OPEN_CALLS {
            assert_eq!(
                Released::default(),
                table
                    .bind_owner(&CallId::tool(format!("c{i}")), owner())
                    .unwrap()
            );
        }
        assert_eq!(MAX_OPEN_CALLS, table.len());

        let one_more = CallId::tool("one-more");
        assert_eq!(
            Released::one(&leftover, Some(0)),
            table.bind_owner(&one_more, owner()).unwrap(),
            "dropped, then released"
        );
        assert!(table.once_rows(&leftover).is_empty(), "its rows went");
        assert_eq!(MAX_OPEN_CALLS, table.len());

        table.stash_once(&CallId::tool("finished"), None, once_grant("late.example"));
        assert_eq!(
            MAX_OPEN_CALLS,
            table.len(),
            "a late call grant creates nothing"
        );
        let refused = CallId::tool("refused");
        assert!(table.bind_owner(&refused, owner()).is_err());
        assert!(
            table.remember(&refused, record(Vec::new())).is_err(),
            "nothing is created past the cap"
        );
        assert_eq!(MAX_OPEN_CALLS, table.len());
        assert!(table.prepared_record(&stray).is_some());
        assert!(table.prepared_record(&background).is_some());
        assert_eq!(Some(SandboxMode::Enforce), table.held_mode(&pinned));
        assert!(
            table.owner_of(&first_owned).is_some(),
            "the oldest running call stays"
        );
    }

    /// The one eviction rule runs in `entry`, so whichever method creates the entry at the cap
    /// makes the room and hands back the call it evicted, for its credential to be revoked: no
    /// caller has to remember to make room first, and none can drop the evicted call unseen.
    #[test]
    fn every_creator_evicts_at_the_cap_and_returns_the_evicted_call() {
        type Create = fn(&mut CallTable, &CallId) -> Result<Released, CallRefused>;
        let creators: [(&str, Create); 4] = [
            ("bind_owner", |table, call| table.bind_owner(call, owner())),
            ("pin_mode", |table, call| {
                table.pin_mode(call, SandboxMode::Enforce)
            }),
            ("remember", |table, call| {
                table.remember(call, record(Vec::new()))
            }),
            ("open_settlement", |table, call| {
                table.open_settlement(call, SandboxMode::Enforce)
            }),
        ];
        for (name, create) in creators {
            let mut table = CallTable::default();
            let leftover = CallId::tool("leftover");
            assert_eq!(
                Released::default(),
                table
                    .open_settlement(&leftover, SandboxMode::Enforce)
                    .unwrap()
            );
            for i in 1..MAX_OPEN_CALLS {
                assert_eq!(
                    Released::default(),
                    table
                        .bind_owner(&CallId::tool(format!("owned-{i}")), owner())
                        .unwrap()
                );
            }
            let newcomer = CallId::tool("newcomer");
            let evicted = create(&mut table, &newcomer)
                .unwrap_or_else(|refused| panic!("{name} refused with room to make: {refused:?}"));
            assert_eq!(Released::one(&leftover, Some(0)), evicted, "{name}");
            assert_eq!(MAX_OPEN_CALLS, table.len(), "{name}");
            assert_eq!(
                Released::default(),
                create(&mut table, &newcomer).unwrap(),
                "{name}: an entry that exists evicts nothing"
            );
        }
    }

    /// A released id is remembered: a spawn or a settlement that lands after the call's final
    /// result is refused instead of re-creating the call, a late call grant is dropped, and a
    /// late refusal note lands nowhere. The hub's dispatch under that id is a new call: it
    /// clears the tombstone and starts a fresh epoch. An eviction tombstones nothing: the stray
    /// it took may spawn again.
    #[test]
    fn released_id_refuses_a_late_spawn_or_settlement_until_the_hub_dispatches_it_again() {
        let mut table = CallTable::default();
        let call = CallId::tool("c1");
        assert_eq!(
            Released::default(),
            table.bind_owner(&call, owner()).unwrap()
        );
        assert_eq!(
            Released::default(),
            table.remember(&call, record(Vec::new())).unwrap()
        );
        assert_eq!(Released::one(&call, Some(0)), table.release(&call).1);
        assert_eq!(
            Released::one(&call, None),
            table.release(&call).1,
            "released again: every epoch, no entry to name one"
        );
        assert_eq!(
            Err(CallRefused::Released),
            table.remember(&call, record(Vec::new()))
        );
        assert_eq!(
            Err(CallRefused::Released),
            table.open_settlement(&call, SandboxMode::Enforce)
        );
        assert!(!table.stash_once(&call, None, once_grant("a.example")));
        table.push_denial(&call, "refused a.example".to_owned());
        assert_eq!(0, table.len(), "nothing re-created the call");

        assert_eq!(
            Released::default(),
            table.pin_mode(&call, SandboxMode::Observe).unwrap(),
            "a new dispatch under the id is a new call"
        );
        assert_eq!(Some(1), table.epoch_of(&call), "with its own epoch");
        assert_eq!(
            Released::default(),
            table.remember(&call, record(Vec::new())).unwrap()
        );
        assert_eq!(Released::one(&call, Some(1)), table.release(&call).1);
        assert_eq!(
            Released::default(),
            table.bind_owner(&call, owner()).unwrap(),
            "bind clears the tombstone as well"
        );

        let mut table = CallTable::default();
        let stray = CallId::tool("stray");
        assert_eq!(
            Released::default(),
            table.remember(&stray, record(Vec::new())).unwrap()
        );
        assert!(table.take_prepared(&stray).0.is_some());
        for i in 1..=MAX_OPEN_CALLS {
            assert!(
                table
                    .bind_owner(&CallId::tool(format!("owned-{i}")), owner())
                    .is_ok()
            );
        }
        assert!(table.epoch_of(&stray).is_none(), "evicted at the cap");
        assert!(
            table.remember(&stray, record(Vec::new())).is_err(),
            "the table is full of owned calls"
        );
        let _ = table.release(&CallId::tool("owned-1"));
        assert_eq!(
            Released::default(),
            table.remember(&stray, record(Vec::new())).unwrap(),
            "an eviction tombstoned nothing"
        );
    }

    /// The tombstones are bounded: past [`MAX_TOMBSTONES`] the oldest is forgotten and a spawn
    /// under it is let in again.
    #[test]
    fn tombstones_forget_the_oldest_past_their_bound() {
        let mut table = CallTable::default();
        let oldest = CallId::tool("oldest");
        let _ = table.release(&oldest);
        assert!(table.remember(&oldest, record(Vec::new())).is_err());
        for i in 0..MAX_TOMBSTONES {
            let _ = table.release(&CallId::tool(format!("later-{i}")));
        }
        assert!(
            table.remember(&oldest, record(Vec::new())).is_ok(),
            "forgotten"
        );
        assert!(
            table
                .remember(&CallId::tool("later-0"), record(Vec::new()))
                .is_err(),
            "the newest bound's worth is kept"
        );
    }

    /// A background start moves the call's epoch on and hands the ended one back for its holds
    /// to be refused, keeping the entry and its credential: a call grant answered on a card from
    /// before the start is dropped, one for the current epoch — or one that names no epoch —
    /// lands. The child's exit releases the entry under the new epoch.
    #[test]
    fn detach_moves_the_epoch_on_and_a_call_grant_from_before_it_is_dropped() {
        let mut table = CallTable::default();
        let call = CallId::tool("c1");
        assert_eq!(
            Released::default(),
            table.bind_owner(&call, owner()).unwrap()
        );
        assert_eq!(
            Released::default(),
            table.remember(&call, record(Vec::new())).unwrap()
        );
        assert_eq!(Some(0), table.epoch_of(&call));
        assert_eq!(
            Released::epoch(&call, 0),
            table.detach(&call, "s-1").1,
            "the epoch ended; the credential stays"
        );
        let after = table.epoch_of(&call).expect("the entry is kept");
        assert!(after > 0);
        assert!(
            !table.stash_once(&call, Some(0), once_grant("late.example")),
            "a card from before the start lands nowhere"
        );
        assert!(table.stash_once(&call, Some(after), once_grant("child.example")));
        assert!(table.stash_once(&call, None, once_grant("now.example")));
        assert_eq!(2, table.once_rows(&call).len());
        assert_eq!(Released::one(&call, Some(after)), table.exited(&call));
        assert_eq!(0, table.len());
    }

    /// The child exits before the hub has the call's start: the spawn's credential and epoch end
    /// there, the entry waits for the result, and the late detach releases the call once.
    #[test]
    fn an_owned_childs_exit_ends_its_epoch_and_the_late_detach_releases_the_call() {
        let mut table = CallTable::default();
        let call = CallId::tool("c1");
        assert_eq!(
            Released::default(),
            table.bind_owner(&call, owner()).unwrap()
        );
        assert_eq!(
            Released::default(),
            table.remember(&call, record(Vec::new())).unwrap()
        );
        assert!(table.stash_once(&call, Some(0), once_grant("child.example")));
        assert_eq!(1, table.once_rows(&call).len());
        assert_eq!(
            Released::one(&call, Some(0)),
            table.exited(&call),
            "the credential goes and the epoch ended"
        );
        assert!(table.owner_of(&call).is_some(), "kept for the hub's result");
        assert_eq!(0, table.prepared());
        assert!(
            table.once_rows(&call).is_empty(),
            "the call-scoped rows went with the spawn, as at detach"
        );
        let after = table.epoch_of(&call).expect("the entry is kept");
        assert!(after > 0);
        assert_eq!(
            Released::default(),
            table.exited(&call),
            "a repeat ends nothing"
        );
        assert_eq!(
            Some(after),
            table.epoch_of(&call),
            "the epoch stays where it moved to"
        );
        assert!(
            !table.stash_once(&call, Some(0), once_grant("late.example")),
            "a card from before the exit lands nowhere"
        );
        assert_eq!(
            Released::one(&call, Some(after)),
            table.detach(&call, "s-1").1,
            "no spawn is left for the late start: the call is released"
        );
        assert_eq!(0, table.len());
        assert_eq!(Released::default(), table.exited(&call));
        assert_eq!(Released::one(&call, None), table.release(&call).1);
        assert_eq!(
            Err(CallRefused::Released),
            table.remember(&call, record(Vec::new())),
            "released once: a late spawn is refused"
        );
    }

    /// A call's mode is kept once held: a second pin for a live call — pinned and not yet
    /// spawned, or with a spawn open — changes neither what it runs under nor what its next
    /// spawn inherits. Only a call the table holds no mode for takes the pin.
    #[test]
    fn pin_keeps_the_mode_a_call_already_holds() {
        let mut table = CallTable::default();
        let pinned = CallId::tool("pinned");
        assert!(table.pin_mode(&pinned, SandboxMode::Enforce).is_ok());
        assert!(table.pin_mode(&pinned, SandboxMode::Observe).is_ok());
        assert_eq!(Some(SandboxMode::Enforce), table.held_mode(&pinned));
        assert_eq!(
            Some(SandboxMode::Enforce),
            table.attempt_settling(&pinned).mode
        );

        let spawned = CallId::tool("spawned");
        assert!(table.bind_owner(&spawned, owner()).is_ok());
        assert!(table.remember(&spawned, record(Vec::new())).is_ok());
        assert!(table.pin_mode(&spawned, SandboxMode::Off).is_ok());
        assert_eq!(Some(SandboxMode::Enforce), table.held_mode(&spawned));
        assert!(table.take_prepared(&spawned).0.is_some());
        assert!(
            table
                .open_settlement(&spawned, SandboxMode::Enforce)
                .is_ok()
        );
        assert_eq!(
            Some(SandboxMode::Enforce),
            table.attempt_settling(&spawned).mode,
            "the replay inherits the spawn's mode, not the late pin"
        );

        let bare = CallId::tool("bare");
        assert!(table.bind_owner(&bare, owner()).is_ok());
        assert_eq!(None, table.held_mode(&bare));
        assert!(table.pin_mode(&bare, SandboxMode::Observe).is_ok());
        assert_eq!(Some(SandboxMode::Observe), table.held_mode(&bare));
    }

    /// What a mode change leaves running under another mode: every live call whose mode is
    /// pinned or spawned differs from the new one; a call with no mode and a finished call count
    /// for nothing.
    #[test]
    fn calls_running_under_another_mode_are_counted_for_the_mode_change_reply() {
        let mut table = CallTable::default();
        let pinned = CallId::tool("pinned");
        let spawned = CallId::tool("spawned");
        let bare = CallId::tool("bare");
        assert!(table.pin_mode(&pinned, SandboxMode::Observe).is_ok());
        assert!(table.remember(&spawned, record(Vec::new())).is_ok());
        assert!(table.bind_owner(&bare, owner()).is_ok());
        assert_eq!(1, table.running_under_another_mode(SandboxMode::Enforce));
        assert_eq!(1, table.running_under_another_mode(SandboxMode::Observe));
        assert_eq!(2, table.running_under_another_mode(SandboxMode::Off));
        assert!(table.any_runs_enforced());
        let _ = table.release(&spawned);
        assert!(!table.any_runs_enforced());
        assert_eq!(0, table.running_under_another_mode(SandboxMode::Observe));
    }

    /// A call floored at `enforce` counts as running enforced until it spawns — the proxy stays
    /// up for it across a flip to `observe` — and for a flip to a weaker mode; a floor no stronger
    /// than the new mode counts for nothing; a call that spawned counts once, by its spawn.
    #[test]
    fn a_floored_call_counts_by_its_floor_until_its_spawn_holds_the_mode() {
        let mut table = CallTable::default();
        let floored = CallId::tool("floored");
        let weaker = CallId::tool("weaker");
        assert!(table.floor_mode(&floored, SandboxMode::Enforce));
        assert!(table.floor_mode(&weaker, SandboxMode::Observe));
        assert!(
            table.any_runs_enforced(),
            "a floored enforce call keeps the proxy"
        );
        assert_eq!(1, table.running_under_another_mode(SandboxMode::Observe));
        assert_eq!(2, table.running_under_another_mode(SandboxMode::Off));
        assert_eq!(
            0,
            table.running_under_another_mode(SandboxMode::Enforce),
            "a floor no stronger than the new mode leaves the call running under the new mode"
        );
        assert!(table.bind_owner(&floored, owner()).is_ok());
        assert_eq!(
            None,
            table.held_mode(&floored),
            "bound, it still holds no mode"
        );
        assert_eq!(1, table.running_under_another_mode(SandboxMode::Observe));
        assert!(table.remember(&floored, record(Vec::new())).is_ok());
        assert_eq!(Some(SandboxMode::Enforce), table.held_mode(&floored));
        assert_eq!(
            1,
            table.running_under_another_mode(SandboxMode::Observe),
            "spawned, it counts once"
        );
        let _ = table.release(&floored);
        assert!(!table.any_runs_enforced());
        assert_eq!(0, table.running_under_another_mode(SandboxMode::Observe));
    }

    /// The table floors as many calls as it holds and refuses one more; a repeat for a floored
    /// call is always room enough and only tightens; a call's release or detach drops its floor.
    #[test]
    fn a_floor_is_refused_past_the_cap_only_tightens_and_goes_with_its_call() {
        let mut table = CallTable::default();
        for i in 0..MAX_OPEN_CALLS {
            assert!(table.floor_mode(&CallId::tool(format!("f{i}")), SandboxMode::Observe));
        }
        assert_eq!(0, table.len(), "a floor is no entry");
        let one_more = CallId::tool("one-more");
        assert!(
            !table.floor_mode(&one_more, SandboxMode::Enforce),
            "refused past the cap"
        );
        assert_eq!(None, table.floor_of(&one_more));
        let first = CallId::tool("f0");
        assert!(
            table.floor_mode(&first, SandboxMode::Enforce),
            "a repeat has room"
        );
        assert_eq!(
            Some(SandboxMode::Enforce),
            table.floor_of(&first),
            "observe then enforce tightens"
        );
        assert!(table.floor_mode(&first, SandboxMode::Observe));
        assert_eq!(
            Some(SandboxMode::Enforce),
            table.floor_of(&first),
            "enforce then observe stays enforce"
        );
        let _ = table.release(&first);
        assert_eq!(None, table.floor_of(&first), "released with the call");
        assert!(
            table.floor_mode(&one_more, SandboxMode::Enforce),
            "the room went with it"
        );
        assert!(table.remember(&one_more, record(Vec::new())).is_ok());
        let _ = table.detach(&one_more, "s-1");
        assert_eq!(
            None,
            table.floor_of(&one_more),
            "detached, the spawn record holds the mode"
        );
        assert_eq!(Some(SandboxMode::Enforce), table.held_mode(&one_more));
    }
}
