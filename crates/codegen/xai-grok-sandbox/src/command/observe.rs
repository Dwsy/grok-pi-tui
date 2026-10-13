//! What `Observe` mode would have blocked: a per-workspace, in-memory, content-free tally the
//! desktop reads through the daemon's control socket. Nothing here blocks, prompts or persists.
//! The rows come from the egress proxy's `AllowAndRecord` decisions (network only); the table
//! keeps the card's whole `blocked.kind` vocabulary so filesystem rows need no new shape.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::command::violation::{Blocked, net_target};

/// Rows kept per workspace; the oldest-seen row is dropped past this.
pub const OBSERVE_SUMMARY_MAX_ROWS: usize = 200;

/// The coarse kind of a would-be denial, the card's `blocked.kind` vocabulary.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, strum::IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum WouldBlockKind {
    FsWrite,
    FsRead,
    Net,
    Capability,
    Unknown,
}

impl WouldBlockKind {
    pub fn of(blocked: &Blocked) -> WouldBlockKind {
        match blocked {
            Blocked::FsWrite { .. } => WouldBlockKind::FsWrite,
            Blocked::FsRead { .. } => WouldBlockKind::FsRead,
            Blocked::Net { .. } => WouldBlockKind::Net,
            Blocked::Capability { .. } => WouldBlockKind::Capability,
            Blocked::Unknown { .. } => WouldBlockKind::Unknown,
        }
    }
}

/// What `enforce` would have done with the observation: the summary shows
/// it per row, so a host the user would have been asked about reads differently from one the
/// folder's rows or the organisation's list would have refused.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, strum::IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum WouldVerdict {
    /// Held and asked about with a card.
    Ask,
    /// Refused silently by a remembered `Deny` row.
    DenyRow,
    /// Refused by `disallowed_web_fetch_domains`, with an informational card.
    PolicyDenylist,
}

/// One would-be denial, aggregated by `(kind, target)` where `target` is what the card would
/// have offered: the host for a connection, the path for a file, the capability's name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WouldBlock {
    pub kind: WouldBlockKind,
    /// The host or `host:port` of a connection, the path of a file; a capability names itself;
    /// unknown is empty.
    pub target: String,
    /// The latest observation's verdict; a row a deny row started covering mid-observe reads as
    /// refused from then on.
    pub verdict: WouldVerdict,
    /// Observations folded into this row.
    pub count: u64,
    /// Unix seconds of the latest observation.
    pub last_at: i64,
}

/// The per-workspace summary. Rows are ordered by most recent observation first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObserveSummary {
    pub would_block: Vec<WouldBlock>,
    /// Observations that arrived once the table was full and displaced an older row.
    #[serde(default)]
    pub evicted: u64,
}

impl ObserveSummary {
    /// Record one would-be denial at `now` (unix seconds), keyed by what the card would name for
    /// it (the host for a connection), with the verdict `enforce` would have reached.
    pub fn record(&mut self, blocked: &Blocked, verdict: WouldVerdict, now: i64) {
        let kind = WouldBlockKind::of(blocked);
        let target = target_of(blocked);
        if let Some(row) = self
            .would_block
            .iter_mut()
            .find(|row| row.kind == kind && row.target == target)
        {
            row.count += 1;
            if now >= row.last_at {
                row.verdict = verdict;
                row.last_at = now;
            }
        } else {
            if self.would_block.len() >= OBSERVE_SUMMARY_MAX_ROWS {
                self.evict_oldest();
            }
            self.would_block.push(WouldBlock {
                kind,
                target,
                verdict,
                count: 1,
                last_at: now,
            });
        }
        self.would_block.sort_by(|a, b| b.last_at.cmp(&a.last_at));
    }

    fn evict_oldest(&mut self) {
        if let Some((index, _)) = self
            .would_block
            .iter()
            .enumerate()
            .min_by_key(|(_, row)| row.last_at)
        {
            self.would_block.remove(index);
            self.evicted += 1;
        }
    }

    /// Row counts by kind, for the metric labels and the banner.
    pub fn counts_by_kind(&self) -> BTreeMap<WouldBlockKind, u64> {
        let mut out = BTreeMap::new();
        for row in &self.would_block {
            *out.entry(row.kind).or_insert(0) += row.count;
        }
        out
    }

    pub fn clear(&mut self) {
        self.would_block.clear();
        self.evicted = 0;
    }
}

fn target_of(blocked: &Blocked) -> String {
    match blocked {
        Blocked::FsWrite { path } | Blocked::FsRead { path } => path.to_string_lossy().into_owned(),
        Blocked::Net { host, port } => net_target(host.as_deref(), *port),
        Blocked::Capability { what } => <&str>::from(*what).to_owned(),
        Blocked::Unknown { .. } => String::new(),
    }
}

#[cfg(test)]
#[path = "observe_tests.rs"]
mod tests;
