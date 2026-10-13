//! Prometheus counters and content-free telemetry events for the per-command sandbox.
//! Zero-initialised from the crate's `init_metrics` so idle panels render a baseline.

use std::sync::LazyLock;

use prometheus::{IntCounterVec, register_int_counter_vec};
use xai_grok_sandbox::command::grants::{Expiry, Grant, GrantScope, GrantSubject};
use xai_grok_sandbox::command::violation::Blocked;
use xai_grok_sandbox::command::{BackendName, SandboxMode, Violation};
use xai_grok_telemetry::events::{
    SandboxBlockedKind, SandboxCommandEnded, SandboxCommandOutcome, SandboxExpiryKind,
    SandboxGrantRecorded, SandboxGrantRevoked, SandboxGrantScopeKind, SandboxGrantSubjectKind,
    SandboxModeTag, SandboxSettlement, SandboxViolationSettled,
};

static COMMAND_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "grok_workspace_sandbox_command_total",
        "Shell commands that went through the per-command sandbox hook, by how they ended",
        &["mode", "backend", "outcome"]
    )
    .expect("grok_workspace_sandbox_command_total must register once")
});

static VIOLATION_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "grok_workspace_sandbox_violation_total",
        "Sandbox denials decoded from a finished command, by kind and how they were settled",
        &["mode", "kind", "settlement"]
    )
    .expect("grok_workspace_sandbox_violation_total must register once")
});

static GRANT_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "grok_workspace_sandbox_grant_total",
        "Grants the user gave on a sandbox card, by subject, scope and expiry",
        &["subject", "scope", "expiry"]
    )
    .expect("grok_workspace_sandbox_grant_total must register once")
});

const MODES: [SandboxMode; 3] = [SandboxMode::Off, SandboxMode::Observe, SandboxMode::Enforce];
const OUTCOMES: [SandboxCommandOutcome; 3] = [
    SandboxCommandOutcome::Ran,
    SandboxCommandOutcome::Violation,
    SandboxCommandOutcome::WrapFailed,
];
const SETTLEMENTS: [SandboxSettlement; 4] = [
    SandboxSettlement::Replay,
    SandboxSettlement::Resume,
    SandboxSettlement::Denied,
    SandboxSettlement::Observed,
];
const KINDS: [SandboxBlockedKind; 5] = [
    SandboxBlockedKind::FsWrite,
    SandboxBlockedKind::FsRead,
    SandboxBlockedKind::Net,
    SandboxBlockedKind::Capability,
    SandboxBlockedKind::Unknown,
];

pub(crate) fn init_metrics() {
    for mode in MODES {
        for outcome in OUTCOMES {
            let _ = COMMAND_TOTAL.with_label_values(&[
                <&str>::from(mode),
                "none",
                <&str>::from(outcome),
            ]);
        }
        for kind in KINDS {
            for settlement in SETTLEMENTS {
                let _ = VIOLATION_TOTAL.with_label_values(&[
                    <&str>::from(mode),
                    <&str>::from(kind),
                    <&str>::from(settlement),
                ]);
            }
        }
    }
    let _ = GRANT_TOTAL.with_label_values(&["fs_write_root", "call", "never"]);
}

/// The `backend` label: the `BackendName` spelling, or `none` on a host with no backend.
fn backend_label(backend: Option<BackendName>) -> &'static str {
    backend.map_or("none", <&'static str>::from)
}

/// The current value of one `grok_workspace_sandbox_command_total` series.
#[cfg(test)]
pub(crate) fn command_total(
    mode: SandboxMode,
    backend: Option<BackendName>,
    outcome: SandboxCommandOutcome,
) -> u64 {
    COMMAND_TOTAL
        .with_label_values(&[
            <&str>::from(mode),
            backend_label(backend),
            <&str>::from(outcome),
        ])
        .get()
}

pub(crate) fn command(
    mode: SandboxMode,
    backend: Option<BackendName>,
    outcome: SandboxCommandOutcome,
) {
    COMMAND_TOTAL
        .with_label_values(&[
            <&str>::from(mode),
            backend_label(backend),
            <&str>::from(outcome),
        ])
        .inc();
    xai_grok_telemetry::log_event(command_ended_event(mode, backend, outcome));
}

pub(crate) fn command_ended_event(
    mode: SandboxMode,
    backend: Option<BackendName>,
    outcome: SandboxCommandOutcome,
) -> SandboxCommandEnded {
    SandboxCommandEnded {
        mode: mode_tag(mode),
        backend: backend.map(<&'static str>::from),
        outcome,
    }
}

/// The current value of one `grok_workspace_sandbox_violation_total` series.
#[cfg(test)]
pub(crate) fn violation_total(
    mode: SandboxMode,
    kind: SandboxBlockedKind,
    settlement: SandboxSettlement,
) -> u64 {
    VIOLATION_TOTAL
        .with_label_values(&[
            <&str>::from(mode),
            <&str>::from(kind),
            <&str>::from(settlement),
        ])
        .get()
}

pub(crate) fn violation(mode: SandboxMode, blocked: &Blocked, settlement: SandboxSettlement) {
    VIOLATION_TOTAL
        .with_label_values(&[
            <&str>::from(mode),
            <&str>::from(blocked_kind(blocked)),
            <&str>::from(settlement),
        ])
        .inc();
}

/// The violation counter plus its telemetry event, once the gate settled it. The series is
/// labelled from the event, so the two carry one spelling.
pub(crate) fn violation_settled(
    mode: SandboxMode,
    violation: &Violation,
    settlement: SandboxSettlement,
) {
    let event = violation_settled_event(mode, violation, settlement);
    VIOLATION_TOTAL
        .with_label_values(&violation_labels(&event))
        .inc();
    xai_grok_telemetry::log_event(event);
}

/// The `grok_workspace_sandbox_violation_total` labels of a settled violation, read off its event.
pub(crate) fn violation_labels(event: &SandboxViolationSettled) -> [&'static str; 3] {
    [
        event.mode.into(),
        event.kind.into(),
        event.settlement.into(),
    ]
}

pub(crate) fn violation_settled_event(
    mode: SandboxMode,
    violation: &Violation,
    settlement: SandboxSettlement,
) -> SandboxViolationSettled {
    SandboxViolationSettled {
        mode: mode_tag(mode),
        kind: blocked_kind(&violation.blocked),
        informational: violation.disposition.reason().map(<&'static str>::from),
        settlement,
    }
}

pub(crate) fn grant(grant: &Grant) {
    GRANT_TOTAL
        .with_label_values(&[
            <&str>::from(subject_kind(&grant.subject)),
            <&str>::from(scope_kind(&grant.scope)),
            <&str>::from(expiry_kind(grant.expires)),
        ])
        .inc();
}

pub(crate) fn grant_recorded_event(grant: &Grant) -> SandboxGrantRecorded {
    SandboxGrantRecorded {
        subject: subject_kind(&grant.subject),
        scope: scope_kind(&grant.scope),
        expiry: expiry_kind(grant.expires),
    }
}

pub(crate) fn grant_revoked_event(scope: &GrantScope) -> SandboxGrantRevoked {
    SandboxGrantRevoked {
        scope: scope_kind(scope),
    }
}

fn blocked_kind(blocked: &Blocked) -> SandboxBlockedKind {
    match blocked {
        Blocked::FsWrite { .. } => SandboxBlockedKind::FsWrite,
        Blocked::FsRead { .. } => SandboxBlockedKind::FsRead,
        Blocked::Net { .. } => SandboxBlockedKind::Net,
        Blocked::Capability { .. } => SandboxBlockedKind::Capability,
        Blocked::Unknown { .. } => SandboxBlockedKind::Unknown,
    }
}

fn mode_tag(mode: SandboxMode) -> SandboxModeTag {
    match mode {
        SandboxMode::Off => SandboxModeTag::Off,
        SandboxMode::Observe => SandboxModeTag::Observe,
        SandboxMode::Enforce => SandboxModeTag::Enforce,
    }
}

fn subject_kind(subject: &GrantSubject) -> SandboxGrantSubjectKind {
    match subject {
        GrantSubject::FsWriteRoot { .. } => SandboxGrantSubjectKind::FsWriteRoot,
        GrantSubject::FsRead { .. } => SandboxGrantSubjectKind::FsRead,
        GrantSubject::NetHost { .. } => SandboxGrantSubjectKind::NetHost,
        GrantSubject::BuildCaches => SandboxGrantSubjectKind::BuildCaches,
    }
}

fn scope_kind(scope: &GrantScope) -> SandboxGrantScopeKind {
    match scope {
        GrantScope::Call => SandboxGrantScopeKind::Call,
        GrantScope::Session => SandboxGrantScopeKind::Session,
        GrantScope::Workspace { .. } => SandboxGrantScopeKind::Workspace,
        GrantScope::Global => SandboxGrantScopeKind::Global,
    }
}

fn expiry_kind(expiry: Expiry) -> SandboxExpiryKind {
    match expiry {
        Expiry::Ttl { .. } => SandboxExpiryKind::Ttl,
        Expiry::At { .. } => SandboxExpiryKind::At,
        Expiry::Never => SandboxExpiryKind::Never,
    }
}
