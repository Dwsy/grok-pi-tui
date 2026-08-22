//! Adapter-owned queue isolation for the native Grok queue surface.
//!
//! User and extension follow-ups and mid-turn steer rows stay here until the
//! adapter dispatches them to Pi. That makes remove/edit/reorder/clear/interject real operations instead of
//! optimistic UI mutations against Pi's text-only RPC arrays. Pi `queue_update`
//! is still mirrored as an external lane for messages that bypass the adapter.

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueLane {
    Steering,
    FollowUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueOrigin {
    Client,
    Extension,
    Pi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueEntry {
    pub id: String,
    pub execution_text: String,
    pub display_text: String,
    pub images: Vec<Value>,
    pub version: u64,
    pub lane: QueueLane,
    pub origin: QueueOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReservedPrompt {
    id: String,
    execution_text: String,
    display_text: String,
    images: Vec<Value>,
    lane: QueueLane,
    origin: QueueOrigin,
}

#[derive(Debug, Default)]
pub(crate) struct QueueMirror {
    local_entries: Vec<QueueEntry>,
    pi_entries: Vec<QueueEntry>,
    reserved: Vec<ReservedPrompt>,
    /// Pi-origin follow-ups that vanished from Pi's queue arrays while the
    /// running slot was owned by another prompt. Promoted by
    /// [`QueueMirror::promote_parked_running`] at the next free `agent_start`.
    parked: Vec<QueueEntry>,
    next_seq: u64,
    running: Option<QueueEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueSnapshot {
    pub entries: Vec<Value>,
    pub running_prompt_id: Option<String>,
    pub running_text: Option<String>,
    pub steering_count: usize,
    pub follow_up_count: usize,
}

impl QueueMirror {
    fn next_id(&mut self, prefix: &str) -> String {
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        format!("{prefix}-{}", self.next_seq)
    }

    pub(crate) fn enqueue_local(
        &mut self,
        id: Option<String>,
        execution_text: String,
        display_text: String,
        images: Vec<Value>,
        lane: QueueLane,
        origin: QueueOrigin,
    ) -> String {
        let id = id.filter(|id| !id.trim().is_empty()).unwrap_or_else(|| {
            let prefix = match origin {
                QueueOrigin::Client => "pi-client-queue",
                QueueOrigin::Extension => "pi-extension-queue",
                QueueOrigin::Pi => "pi-queue",
            };
            self.next_id(prefix)
        });
        if let Some(existing) = self.local_entries.iter_mut().find(|entry| entry.id == id) {
            existing.execution_text = execution_text;
            existing.display_text = display_text;
            existing.images = images;
            existing.lane = lane;
            existing.origin = origin;
            return id;
        }
        self.local_entries.push(QueueEntry {
            id: id.clone(),
            execution_text,
            display_text,
            images,
            version: 0,
            lane,
            origin,
        });
        id
    }

    pub(crate) fn take_local(
        &mut self,
        id: &str,
        expected_version: Option<u64>,
    ) -> Option<QueueEntry> {
        let index = self.local_entries.iter().position(|entry| {
            entry.id == id && expected_version.is_none_or(|version| version == entry.version)
        })?;
        Some(self.local_entries.remove(index))
    }

    pub(crate) fn pop_next_local(&mut self) -> Option<QueueEntry> {
        (!self.local_entries.is_empty()).then(|| self.local_entries.remove(0))
    }

    /// Oldest pending local row in `lane`, as `(id, version)` for an atomic
    /// [`QueueMirror::take_local`].
    pub(crate) fn next_local_in_lane(&self, lane: QueueLane) -> Option<(String, u64)> {
        self.local_entries
            .iter()
            .find(|entry| entry.lane == lane)
            .map(|entry| (entry.id.clone(), entry.version))
    }

    pub(crate) fn push_front_local(&mut self, entry: QueueEntry) {
        self.local_entries.retain(|current| current.id != entry.id);
        self.local_entries.insert(0, entry);
    }

    pub(crate) fn edit_local(&mut self, id: &str, new_text: String) -> bool {
        let Some(entry) = self.local_entries.iter_mut().find(|entry| entry.id == id) else {
            return false;
        };
        entry.execution_text = new_text.clone();
        entry.display_text = new_text;
        entry.version = entry.version.wrapping_add(1);
        true
    }

    pub(crate) fn reorder_local(&mut self, ordered_ids: &[String]) -> bool {
        if self.local_entries.len() < 2 {
            return false;
        }
        let before: Vec<String> = self
            .local_entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect();
        let mut remaining = std::mem::take(&mut self.local_entries);
        let mut reordered = Vec::with_capacity(remaining.len());
        for id in ordered_ids {
            if let Some(index) = remaining.iter().position(|entry| &entry.id == id) {
                reordered.push(remaining.remove(index));
            }
        }
        reordered.append(&mut remaining);
        let after: Vec<String> = reordered.iter().map(|entry| entry.id.clone()).collect();
        self.local_entries = reordered;
        before != after
    }

    pub(crate) fn clear_local(&mut self) -> Vec<QueueEntry> {
        std::mem::take(&mut self.local_entries)
    }

    pub(crate) fn reserve(
        &mut self,
        id: String,
        execution_text: String,
        display_text: String,
        images: Vec<Value>,
        lane: QueueLane,
        origin: QueueOrigin,
    ) {
        if id.trim().is_empty() {
            return;
        }
        self.reserved.retain(|item| item.id != id);
        self.reserved.push(ReservedPrompt {
            id,
            execution_text,
            display_text,
            images,
            lane,
            origin,
        });
    }

    pub(crate) fn release_reservation(&mut self, id: &str) {
        self.reserved.retain(|item| item.id != id);
    }

    pub(crate) fn set_running(&mut self, entry: QueueEntry) {
        self.running = Some(entry);
    }

    pub(crate) fn set_running_primary(
        &mut self,
        id: String,
        execution_text: String,
        display_text: String,
        images: Vec<Value>,
        origin: QueueOrigin,
    ) {
        if id.trim().is_empty() {
            return;
        }
        self.running = Some(QueueEntry {
            id,
            execution_text,
            display_text,
            images,
            version: 0,
            lane: QueueLane::FollowUp,
            origin,
        });
    }

    pub(crate) fn running(&self) -> Option<&QueueEntry> {
        self.running.as_ref()
    }

    pub(crate) fn clear_running(&mut self) -> Option<QueueEntry> {
        self.reserved.clear();
        self.running.take()
    }

    pub(crate) fn apply_queue_update(
        &mut self,
        steering: &[String],
        follow_up: &[String],
    ) -> QueueSnapshot {
        let desired: Vec<(String, QueueLane)> = steering
            .iter()
            .cloned()
            .map(|text| (text, QueueLane::Steering))
            .chain(
                follow_up
                    .iter()
                    .cloned()
                    .map(|text| (text, QueueLane::FollowUp)),
            )
            .collect();

        let previous = std::mem::take(&mut self.pi_entries);
        let mut next = Vec::with_capacity(desired.len());
        let mut used_prev = vec![false; previous.len()];

        for (text, lane) in &desired {
            if let Some((index, entry)) = previous.iter().enumerate().find(|(index, entry)| {
                !used_prev[*index] && entry.lane == *lane && entry.execution_text == *text
            }) {
                used_prev[index] = true;
                next.push(entry.clone());
                continue;
            }

            let reserved_index = self
                .reserved
                .iter()
                .position(|item| item.lane == *lane && item.execution_text == *text)
                .or_else(|| self.reserved.iter().position(|item| item.lane == *lane));
            if let Some(index) = reserved_index {
                let reserved = self.reserved.remove(index);
                next.push(QueueEntry {
                    id: reserved.id,
                    execution_text: text.clone(),
                    display_text: reserved.display_text,
                    images: reserved.images,
                    version: 0,
                    lane: *lane,
                    origin: reserved.origin,
                });
                continue;
            }

            let id = self.next_id("pi-queue");
            next.push(QueueEntry {
                id,
                execution_text: text.clone(),
                display_text: text.clone(),
                images: Vec::new(),
                version: 0,
                lane: *lane,
                origin: QueueOrigin::Pi,
            });
        }

        self.reserved
            .retain(|item| !next.iter().any(|entry| entry.id == item.id));

        // A follow-up that vanished from Pi's arrays started executing inside
        // Pi. Promote it to running ONLY when the slot is free: stealing the
        // slot from a live entry (e.g. the client `/goal` command prompt that
        // queued this follow-up via sendUserMessage) loses that entry's
        // identity — its settle then emits prompt_complete for a pid nobody
        // adopted, and the real turn's settle finds an empty slot, so no
        // terminal signal ever reaches the pager and it strands on
        // "Waiting for response…". Park displaced leftovers instead;
        // agent_start claims them once the slot frees.
        let dequeued_follow_ups: Vec<QueueEntry> = previous
            .iter()
            .enumerate()
            .filter(|(index, entry)| !used_prev[*index] && entry.lane == QueueLane::FollowUp)
            .map(|(_, entry)| entry.clone())
            .collect();
        match (self.running.as_ref(), dequeued_follow_ups.split_first()) {
            (None, Some((first, rest))) => {
                self.running = Some(first.clone());
                self.parked.extend(rest.iter().cloned());
            }
            (_, _) => self.parked.extend(dequeued_follow_ups),
        }
        self.pi_entries = next;
        self.snapshot()
    }

    /// Claim a parked Pi-origin follow-up as running at a turn boundary.
    /// Called on `agent_start` when no tracked prompt owns the slot, so a turn
    /// Pi chained from its own queue still broadcasts a running id and receives
    /// a matching `prompt_complete` at settle.
    pub(crate) fn promote_parked_running(&mut self) -> bool {
        if self.running.is_some() {
            return false;
        }
        while let Some(entry) = self.parked.pop() {
            let requeued = self
                .pi_entries
                .iter()
                .chain(self.local_entries.iter())
                .any(|current| current.id == entry.id);
            if requeued {
                continue;
            }
            self.running = Some(entry);
            return true;
        }
        false
    }

    pub(crate) fn snapshot(&self) -> QueueSnapshot {
        let entries = self
            .pi_entries
            .iter()
            .chain(self.local_entries.iter())
            .enumerate()
            .map(|(position, entry)| {
                json!({
                    "id": entry.id,
                    "version": entry.version,
                    "kind": "prompt",
                    "text": entry.display_text,
                    "position": position,
                })
            })
            .collect();
        let steering_count = self
            .pi_entries
            .iter()
            .chain(self.local_entries.iter())
            .filter(|entry| entry.lane == QueueLane::Steering)
            .count();
        // Parked entries are not listed as queue rows, but Pi will still run
        // them — count them so continuation guards (e.g. maybe_continue_goal)
        // do not stack a duplicate directive on top of a pending reminder.
        let total = self.pi_entries.len() + self.local_entries.len() + self.parked.len();
        QueueSnapshot {
            entries,
            running_prompt_id: self.running.as_ref().map(|entry| entry.id.clone()),
            running_text: self
                .running
                .as_ref()
                .map(|entry| entry.display_text.clone()),
            steering_count,
            follow_up_count: total - steering_count,
        }
    }

    pub(crate) fn clear(&mut self) -> QueueSnapshot {
        self.local_entries.clear();
        self.pi_entries.clear();
        self.reserved.clear();
        self.parked.clear();
        self.running = None;
        self.snapshot()
    }
}

pub(crate) fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn queue_changed_params(session_id: &str, snapshot: &QueueSnapshot) -> Value {
    let mut params = json!({
        "sessionId": session_id,
        "entries": snapshot.entries,
    });
    if let Some(running) = &snapshot.running_prompt_id {
        params["runningPromptId"] = Value::String(running.clone());
        params["runningKind"] = Value::String("prompt".to_string());
    }
    if let Some(text) = &snapshot.running_text {
        params["runningText"] = Value::String(text.clone());
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enqueue(mirror: &mut QueueMirror, id: &str, text: &str) {
        mirror.enqueue_local(
            Some(id.into()),
            text.into(),
            text.into(),
            Vec::new(),
            QueueLane::FollowUp,
            QueueOrigin::Client,
        );
    }

    #[test]
    fn local_queue_supports_edit_reorder_remove_and_clear() {
        let mut mirror = QueueMirror::default();
        enqueue(&mut mirror, "a", "one");
        enqueue(&mut mirror, "b", "two");
        enqueue(&mut mirror, "c", "three");
        assert!(mirror.edit_local("b", "two edited".into()));
        assert_eq!(
            mirror.local_entries.iter().find(|e| e.id == "b").unwrap().version,
            1
        );
        assert!(mirror.reorder_local(&["c".into(), "b".into(), "a".into()]));
        assert_eq!(mirror.snapshot().entries[0]["id"], "c");
        let removed = mirror.take_local("b", Some(1)).unwrap();
        assert_eq!(removed.display_text, "two edited");
        assert_eq!(mirror.clear_local().len(), 2);
        assert!(mirror.snapshot().entries.is_empty());
    }

    #[test]
    fn running_entry_carries_text_for_pager_adoption() {
        let mut mirror = QueueMirror::default();
        enqueue(&mut mirror, "client-1", "hello");
        let entry = mirror.pop_next_local().unwrap();
        mirror.set_running(entry);
        let snapshot = mirror.snapshot();
        assert_eq!(snapshot.running_prompt_id.as_deref(), Some("client-1"));
        assert_eq!(snapshot.running_text.as_deref(), Some("hello"));
        let params = queue_changed_params("session", &snapshot);
        assert_eq!(params["runningText"], "hello");
    }

    #[test]
    fn pi_updates_do_not_erase_local_queue() {
        let mut mirror = QueueMirror::default();
        enqueue(&mut mirror, "local", "later");
        mirror.apply_queue_update(&["steer".into()], &["external".into()]);
        let snapshot = mirror.snapshot();
        assert_eq!(snapshot.entries.len(), 3);
        assert_eq!(snapshot.entries[2]["id"], "local");
    }

    #[test]
    fn transformed_pi_text_keeps_reserved_id_and_display() {
        let mut mirror = QueueMirror::default();
        mirror.reserve(
            "client-1".into(),
            "raw input".into(),
            "raw input".into(),
            Vec::new(),
            QueueLane::Steering,
            QueueOrigin::Client,
        );
        let snapshot = mirror.apply_queue_update(&["expanded input".into()], &[]);
        assert_eq!(snapshot.entries[0]["id"], "client-1");
        assert_eq!(snapshot.entries[0]["text"], "raw input");
    }

    #[test]
    fn held_steering_rows_are_cancellable_and_lane_scoped() {
        let mut mirror = QueueMirror::default();
        mirror.enqueue_local(
            Some("s1".into()),
            "steer one".into(),
            "steer one".into(),
            Vec::new(),
            QueueLane::Steering,
            QueueOrigin::Client,
        );
        mirror.enqueue_local(
            Some("f1".into()),
            "follow".into(),
            "follow".into(),
            Vec::new(),
            QueueLane::FollowUp,
            QueueOrigin::Client,
        );
        // Flush picks the oldest steering row, ignoring the follow-up lane.
        assert_eq!(
            mirror.next_local_in_lane(QueueLane::Steering),
            Some(("s1".into(), 0))
        );
        // Cancel = atomic take; version mismatch must reject.
        assert!(mirror.take_local("s1", Some(1)).is_none());
        let cancelled = mirror.take_local("s1", Some(0)).unwrap();
        assert_eq!(cancelled.lane, QueueLane::Steering);
        assert_eq!(mirror.next_local_in_lane(QueueLane::Steering), None);
        assert_eq!(mirror.next_local_in_lane(QueueLane::FollowUp).unwrap().0, "f1");
    }

    #[test]
    fn external_follow_up_dequeue_becomes_running() {
        let mut mirror = QueueMirror::default();
        mirror.apply_queue_update(&[], &["one".into(), "two".into()]);
        let first_ids: Vec<String> = mirror
            .pi_entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect();
        mirror.apply_queue_update(&[], &["two".into()]);
        assert_eq!(mirror.running().unwrap().id, first_ids[0]);
        assert_eq!(mirror.running().unwrap().origin, QueueOrigin::Pi);
    }

    #[test]
    fn steering_dequeue_preserves_primary_running_entry() {
        let mut mirror = QueueMirror::default();
        mirror.set_running_primary(
            "primary".into(),
            "primary".into(),
            "primary".into(),
            Vec::new(),
            QueueOrigin::Client,
        );
        mirror.apply_queue_update(&["change".into()], &[]);
        mirror.apply_queue_update(&[], &[]);
        assert_eq!(mirror.running().unwrap().id, "primary");
    }

    /// Regression: a Pi-queued follow-up that starts executing while a client
    /// prompt owns the running slot (the `/goal` flow — the command prompt is
    /// still settling when Pi chains the queued GOAL reminder) must NOT steal
    /// the slot. Stealing lost the client entry's identity, so the command
    /// settle emitted prompt_complete for the pi-queue id prematurely and the
    /// real turn's settle found an empty slot — no terminal signal ever reached
    /// the pager, which stayed on "Waiting for response…" forever.
    #[test]
    fn follow_up_dequeue_never_steals_a_live_running_slot() {
        let mut mirror = QueueMirror::default();
        mirror.set_running_primary(
            "client-1".into(),
            "/goal ship it".into(),
            "/goal ship it".into(),
            Vec::new(),
            QueueOrigin::Client,
        );
        mirror.apply_queue_update(&[], &["GOAL reminder".into()]);
        mirror.apply_queue_update(&[], &[]);

        assert_eq!(mirror.running().unwrap().id, "client-1");
        assert_eq!(mirror.running().unwrap().origin, QueueOrigin::Client);
        // Slot occupied: parked entries stay parked…
        assert!(!mirror.promote_parked_running());
        assert_eq!(mirror.running().unwrap().id, "client-1");
        // …but still count as pending work for continuation guards.
        assert_eq!(mirror.snapshot().follow_up_count, 1);
        assert!(mirror.snapshot().entries.is_empty());
    }

    #[test]
    fn parked_follow_up_promotes_once_the_running_slot_frees() {
        let mut mirror = QueueMirror::default();
        mirror.set_running_primary(
            "client-1".into(),
            "cmd".into(),
            "cmd".into(),
            Vec::new(),
            QueueOrigin::Client,
        );
        mirror.apply_queue_update(&[], &["reminder".into()]);
        mirror.apply_queue_update(&[], &[]);
        mirror.clear_running();

        assert!(mirror.promote_parked_running());
        let snapshot = mirror.snapshot();
        assert_eq!(snapshot.running_prompt_id.as_deref(), Some("pi-queue-1"));
        assert_eq!(mirror.running().unwrap().origin, QueueOrigin::Pi);
        // Claimed entry keeps its 1:1 pairing: one clear_running → one
        // prompt_complete at settle.
        let claimed = mirror.clear_running().unwrap();
        assert_eq!(claimed.id, "pi-queue-1");
        assert!(!mirror.promote_parked_running());
    }

    #[test]
    fn generated_extension_ids_are_stable_and_distinct() {
        let mut mirror = QueueMirror::default();
        let first = mirror.enqueue_local(
            None,
            "one".into(),
            "one".into(),
            Vec::new(),
            QueueLane::FollowUp,
            QueueOrigin::Extension,
        );
        let second = mirror.enqueue_local(
            None,
            "two".into(),
            "two".into(),
            Vec::new(),
            QueueLane::FollowUp,
            QueueOrigin::Extension,
        );
        assert!(first.starts_with("pi-extension-queue-"));
        assert_ne!(first, second);
    }
}
