//! Build the strategy report from a dispatch outcome plus the request gate.

use xai_fast_worktree::{ArmSkip, GroveHardFail, GroveSkip, WorktreeReport};
use xai_grok_telemetry::events::{
    CloneCancellationDisposition, CloneDaemonCapabilityClass, CloneFallbackReason, CloneOutcome,
    CloneStrategy, CloneTransport, WorktreeEnded, WorktreeLifecycle,
};
use xai_grok_telemetry::session_ctx::log_event;
use xai_grok_workspace_types::rpc::worktree::{
    StrategyReport, WorktreeType, is_grove_resolved, transport_for_resolved,
};

pub(super) struct WorktreeEndedEmit<'a> {
    pub lifecycle: WorktreeLifecycle,
    pub outcome: CloneOutcome,
    pub duration_ms: u64,
    pub grove_enabled: bool,
    pub grove_gate_source: Option<&'a str>,
    pub requested_type: WorktreeType,
    pub rewrite_reason: Option<&'a str>,
    pub worktree: Option<&'a WorktreeReport>,
    pub cancellation_disposition: Option<CloneCancellationDisposition>,
    pub builder_failure: Option<CloneFallbackReason>,
}

pub(super) fn emit_worktree_ended(emit: WorktreeEndedEmit<'_>) -> WorktreeEnded {
    let report = emit.worktree.map(|worktree| {
        report_from_worktree(
            emit.grove_enabled,
            emit.grove_gate_source,
            emit.requested_type,
            emit.rewrite_reason,
            worktree,
        )
    });
    let requested = report
        .as_ref()
        .and_then(|r| r.requested_strategy.as_deref())
        .unwrap_or_else(|| {
            requested_strategy(
                emit.grove_enabled,
                emit.grove_gate_source,
                emit.requested_type,
            )
        });
    let event = WorktreeEnded {
        lifecycle: emit.lifecycle,
        duration_ms: emit.duration_ms,
        outcome: emit.outcome,
        transport: report
            .as_ref()
            .and_then(|r| r.transport.as_deref())
            .and_then(CloneTransport::from_transport_str),
        requested_strategy: CloneStrategy::from_strategy_str(requested),
        resolved_strategy: report
            .as_ref()
            .and_then(|r| r.resolved_strategy.as_deref())
            .and_then(CloneStrategy::from_strategy_str),
        fallback_reason: classify_fallback(
            emit.grove_enabled,
            emit.grove_gate_source,
            emit.worktree.map(|w| w.resolved_strategy),
            emit.rewrite_reason,
            emit.worktree.map(|w| w.skipped.as_slice()).unwrap_or(&[]),
            emit.builder_failure,
        ),
        cancellation_disposition: emit.cancellation_disposition,
        daemon_capability_class: report
            .as_ref()
            .and_then(|r| r.daemon_capability_class.as_deref())
            .and_then(CloneDaemonCapabilityClass::from_class_str),
    };
    log_event(event.clone());
    #[cfg(test)]
    LAST_WORKTREE_ENDED.with(|slot| {
        *slot.borrow_mut() = Some(event.clone());
    });
    event
}

fn classify_fallback(
    grove_enabled: bool,
    grove_gate_source: Option<&str>,
    resolved: Option<&str>,
    rewrite_reason: Option<&str>,
    skipped: &[ArmSkip],
    builder_failure: Option<CloneFallbackReason>,
) -> Option<CloneFallbackReason> {
    if !grove_enabled {
        return match grove_gate_source {
            Some("remote_kill") => Some(CloneFallbackReason::RemoteKill),
            Some("remote_unavailable") => Some(CloneFallbackReason::RemoteUnavailable),
            _ => None,
        };
    }
    if builder_failure.is_some() {
        return builder_failure;
    }
    if resolved.is_some_and(is_grove_resolved) {
        return None;
    }
    if let Some(skip) = skipped.iter().find(|s| s.arm.is_grove()) {
        return Some(match skip.grove_skip {
            Some(grove) => map_grove_skip(grove),
            None => CloneFallbackReason::Other,
        });
    }
    if rewrite_reason == Some(xai_fast_worktree::SKIP_SOURCE_IS_GROVE_MOUNT) {
        return Some(CloneFallbackReason::SourceIsGrove);
    }
    None
}

pub(super) fn emit_failed_without_report(
    lifecycle: WorktreeLifecycle,
    grove_enabled: bool,
    grove_gate_source: Option<&str>,
    requested_type: WorktreeType,
) {
    emit_worktree_ended(WorktreeEndedEmit {
        lifecycle,
        outcome: CloneOutcome::Failed,
        duration_ms: 0,
        grove_enabled,
        grove_gate_source,
        requested_type,
        rewrite_reason: None,
        worktree: None,
        cancellation_disposition: None,
        builder_failure: None,
    });
}

pub(super) fn fallback_from_builder_err(err: &anyhow::Error) -> Option<CloneFallbackReason> {
    match xai_fast_worktree::grove_hard_fail(err) {
        Some(GroveHardFail::InFlight) => Some(CloneFallbackReason::InFlight),
        Some(
            GroveHardFail::StorageFull
            | GroveHardFail::IdentityConflict
            | GroveHardFail::DestStillMounted,
        ) => Some(CloneFallbackReason::Other),
        None => None,
    }
}

fn map_grove_skip(skip: GroveSkip) -> CloneFallbackReason {
    match skip {
        #[cfg(target_os = "linux")]
        GroveSkip::FuseUnavailable => CloneFallbackReason::FuseUnavailable,
        #[cfg(target_os = "linux")]
        GroveSkip::PrivateMountNamespace => CloneFallbackReason::Other,
        GroveSkip::SourceIsGroveMount => CloneFallbackReason::SourceIsGrove,
        GroveSkip::DaemonDeclined => CloneFallbackReason::DaemonDeclined,
        GroveSkip::PreserveOnLinkedView
        | GroveSkip::MountTableInconclusive
        | GroveSkip::PreserveOnInconclusiveLinkedView
        | GroveSkip::JjSourceRepo
        | GroveSkip::PreserveNonHeadRef
        | GroveSkip::HeadUnreadableAfterAdopt => CloneFallbackReason::Other,
    }
}

#[cfg(test)]
std::thread_local! {
    static LAST_WORKTREE_ENDED: std::cell::RefCell<Option<WorktreeEnded>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn last_worktree_ended_for_test() -> Option<WorktreeEnded> {
    LAST_WORKTREE_ENDED.with(|slot| slot.borrow_mut().take())
}

pub(super) fn report_from_worktree(
    grove_enabled: bool,
    grove_gate_source: Option<&str>,
    requested_type: WorktreeType,
    rewrite_reason: Option<&str>,
    report: &WorktreeReport,
) -> StrategyReport {
    let requested = requested_strategy(grove_enabled, grove_gate_source, requested_type);
    let resolved = report.resolved_strategy;
    let grove_transport = report
        .strategy_metadata
        .as_ref()
        .and_then(|m| m.get("grove"))
        .and_then(|g| g.get("transport"))
        .and_then(|v| v.as_str());
    StrategyReport {
        requested_strategy: Some(requested.into()),
        resolved_strategy: Some(resolved.into()),
        transport: transport_for_resolved(resolved, grove_transport).map(str::to_owned),
        // The adopt reply carries no object source; only the clone path knows it.
        source_mode: None,
        fallback_reason: fallback_reason(
            grove_enabled,
            grove_gate_source,
            resolved,
            rewrite_reason,
            &report.skipped,
        ),
        daemon_capability_class: report.daemon_capability_class.map(str::to_owned),
    }
}

fn requested_strategy(
    grove_enabled: bool,
    grove_gate_source: Option<&str>,
    requested_type: WorktreeType,
) -> &'static str {
    if grove_enabled
        || matches!(
            grove_gate_source,
            Some("remote_kill" | "remote_unavailable")
        )
    {
        return "grove";
    }
    match requested_type {
        WorktreeType::Linked => "linked",
        WorktreeType::Standalone => "standalone",
        WorktreeType::Git => "git",
    }
}

fn fallback_reason(
    grove_enabled: bool,
    grove_gate_source: Option<&str>,
    resolved: &str,
    rewrite_reason: Option<&str>,
    skipped: &[ArmSkip],
) -> Option<String> {
    if !grove_enabled {
        return match grove_gate_source {
            Some("remote_kill") => Some("remote Grove is off".into()),
            Some("remote_unavailable") => Some("remote Grove is unavailable".into()),
            _ => None,
        };
    }
    if is_grove_resolved(resolved) {
        return None;
    }
    // Only a grove skip explains why grove did not run: a snapshot arm's skip
    // means an earlier arm won and grove was never reached.
    skipped
        .iter()
        .find(|s| s.arm.is_grove())
        .map(ArmSkip::to_string)
        // A pre-dispatch rewrite kept every arm from running, so it is the only
        // account of why grove did not serve this worktree.
        .or_else(|| rewrite_reason.map(str::to_owned))
}

pub(super) fn creating_progress(grove_enabled: bool) -> &'static str {
    if grove_enabled {
        "Creating worktree with Grove..."
    } else {
        "Creating worktree..."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use xai_fast_worktree::{CopyReport, WorktreeArm};

    fn report(
        resolved: &'static str,
        skipped: Vec<ArmSkip>,
        daemon: Option<&'static str>,
    ) -> WorktreeReport {
        WorktreeReport {
            worktree_path: PathBuf::from("/wt"),
            commit: "abc".into(),
            unignored_copy: CopyReport::default(),
            ignored_copy: None,
            resolved_strategy: resolved,
            strategy_metadata: None,
            skipped,
            daemon_capability_class: daemon,
        }
    }

    #[test]
    fn grove_success_and_copy_fallback_print_the_right_reason() {
        let ok = report_from_worktree(true, Some("request"), WorktreeType::Linked, None, &{
            let mut r = report("grove-fuse", Vec::new(), Some("current"));
            r.strategy_metadata = Some(serde_json::json!({"grove":{"transport":"fuse"}}));
            r
        });
        assert_eq!(ok.requested_strategy.as_deref(), Some("grove"));
        assert_eq!(ok.resolved_strategy.as_deref(), Some("grove-fuse"));
        assert_eq!(ok.transport.as_deref(), Some("fuse"));
        assert_eq!(ok.source_mode, None);
        assert!(ok.fallback_reason.is_none());
        assert_eq!(ok.summary(), "Requested Grove; using `grove-fuse`.");

        let copy = report_from_worktree(
            false,
            Some("remote_kill"),
            WorktreeType::Linked,
            None,
            &report("copy", Vec::new(), Some("unknown")),
        );
        assert_eq!(copy.requested_strategy.as_deref(), Some("grove"));
        assert_eq!(copy.resolved_strategy.as_deref(), Some("copy"));
        assert_eq!(copy.fallback_reason.as_deref(), Some("remote Grove is off"));
        assert_eq!(
            copy.summary(),
            "Requested Grove; using copy because remote Grove is off."
        );

        let skipped = report_from_worktree(
            true,
            Some("request"),
            WorktreeType::Linked,
            None,
            &report(
                "copy",
                vec![ArmSkip::new(
                    WorktreeArm::GroveFuse,
                    "/dev/fuse or fusermount missing",
                )],
                Some("unknown"),
            ),
        );
        assert_eq!(
            skipped.fallback_reason.as_deref(),
            Some("grove-fuse: /dev/fuse or fusermount missing")
        );
        assert!(skipped.summary().contains("grove-fuse:"));

        let old_and_skip = report_from_worktree(
            true,
            Some("request"),
            WorktreeType::Linked,
            None,
            &report(
                "copy",
                vec![ArmSkip::new(
                    WorktreeArm::GroveFuse,
                    "/dev/fuse or fusermount missing",
                )],
                Some("old"),
            ),
        );
        assert_eq!(
            old_and_skip.fallback_reason.as_deref(),
            Some("grove-fuse: /dev/fuse or fusermount missing")
        );
        assert!(old_and_skip.summary().contains("grove-fuse:"));
        assert!(!old_and_skip.summary().contains("daemon too old"));

        let overlay = report_from_worktree(
            true,
            Some("request"),
            WorktreeType::Linked,
            None,
            &report("overlay", Vec::new(), Some("old")),
        );
        assert_eq!(overlay.resolved_strategy.as_deref(), Some("overlay"));
        assert_eq!(overlay.daemon_capability_class.as_deref(), Some("old"));
        assert!(overlay.fallback_reason.is_none());
        assert_eq!(overlay.summary(), "Requested Grove; using overlay.");
    }

    #[test]
    fn earlier_arm_skip_lines_are_not_a_grove_fallback_reason() {
        let btrfs_won = report_from_worktree(
            true,
            Some("request"),
            WorktreeType::Linked,
            None,
            &report(
                "btrfs",
                vec![ArmSkip::new(WorktreeArm::Overlay, "mount failed: EPERM")],
                Some("current"),
            ),
        );
        assert!(
            btrfs_won.fallback_reason.is_none(),
            "grove never ran, so an overlay error cannot explain its absence: {:?}",
            btrfs_won.fallback_reason
        );
        assert_eq!(btrfs_won.summary(), "Requested Grove; using btrfs.");

        let copy_after_grove = report_from_worktree(
            true,
            Some("request"),
            WorktreeType::Linked,
            None,
            &report(
                "copy",
                vec![
                    ArmSkip::new(WorktreeArm::Overlay, "mount failed: EPERM"),
                    ArmSkip::new(WorktreeArm::GroveFuse, "daemon declined or unreachable"),
                ],
                Some("current"),
            ),
        );
        assert_eq!(
            copy_after_grove.fallback_reason.as_deref(),
            Some("grove-fuse: daemon declined or unreachable")
        );
    }

    #[test]
    fn a_pre_dispatch_rewrite_still_explains_itself() {
        // The source was already a Grove mount, so the workspace rewrote the mode
        // to git before dispatch: no arm ran, and no arm recorded a skip.
        let rewritten = report_from_worktree(
            true,
            Some("request"),
            WorktreeType::Linked,
            Some(xai_fast_worktree::SKIP_SOURCE_IS_GROVE_MOUNT),
            &report("git", Vec::new(), None),
        );
        assert_eq!(
            rewritten.summary(),
            "Requested Grove; using git because source is itself a Grove mount."
        );

        // A grove skip line still wins: it is the arm's own account.
        let both = report_from_worktree(
            true,
            Some("request"),
            WorktreeType::Linked,
            Some(xai_fast_worktree::SKIP_SOURCE_IS_GROVE_MOUNT),
            &report(
                "copy",
                vec![ArmSkip::new(
                    WorktreeArm::GroveFuse,
                    "the source is a jj repo",
                )],
                None,
            ),
        );
        assert_eq!(
            both.fallback_reason.as_deref(),
            Some("grove-fuse: the source is a jj repo")
        );
    }

    #[test]
    fn notice_is_silent_for_an_ordinary_copy_worktree() {
        let plain = report_from_worktree(
            false,
            Some("default"),
            WorktreeType::Linked,
            None,
            &report("copy", Vec::new(), None),
        );
        assert_eq!(plain.summary(), "Using copy.");
        assert_eq!(plain.notice(), None);

        let grove = report_from_worktree(
            true,
            Some("request"),
            WorktreeType::Linked,
            None,
            &report(
                "copy",
                vec![ArmSkip::new(
                    WorktreeArm::GroveFuse,
                    "private mount namespace",
                )],
                None,
            ),
        );
        assert_eq!(
            grove.notice().as_deref(),
            Some("Requested Grove; using copy because grove-fuse: private mount namespace.")
        );
    }

    fn emit_from(
        lifecycle: WorktreeLifecycle,
        outcome: CloneOutcome,
        grove_enabled: bool,
        gate: Option<&str>,
        rewrite: Option<&str>,
        worktree: Option<&WorktreeReport>,
        cancellation: Option<CloneCancellationDisposition>,
    ) -> WorktreeEnded {
        emit_worktree_ended(WorktreeEndedEmit {
            lifecycle,
            outcome,
            duration_ms: 9,
            grove_enabled,
            grove_gate_source: gate,
            requested_type: WorktreeType::Linked,
            rewrite_reason: rewrite,
            worktree,
            cancellation_disposition: cancellation,
            builder_failure: None,
        })
    }

    #[test]
    fn worktree_ended_success_has_strategy_transport_daemon_and_no_fallback() {
        let mut wt = report("grove-fuse", Vec::new(), Some("current"));
        wt.strategy_metadata = Some(serde_json::json!({"grove":{"transport":"fuse"}}));
        let event = emit_from(
            WorktreeLifecycle::Create,
            CloneOutcome::Success,
            true,
            Some("request"),
            None,
            Some(&wt),
            None,
        );
        assert_eq!(event.lifecycle, WorktreeLifecycle::Create);
        assert_eq!(event.outcome, CloneOutcome::Success);
        assert_eq!(event.requested_strategy, Some(CloneStrategy::Grove));
        assert_eq!(event.resolved_strategy, Some(CloneStrategy::GroveFuse));
        assert_eq!(event.transport, Some(CloneTransport::Fuse));
        assert_eq!(
            event.daemon_capability_class,
            Some(CloneDaemonCapabilityClass::Current)
        );
        assert_eq!(event.fallback_reason, None);
        let json = serde_json::to_value(&event).unwrap();
        assert!(json.get("fallback_reason").is_none());
        assert!(json.get("source_mode").is_none());
    }

    #[test]
    fn worktree_ended_grove_to_copy_uses_closed_fallback_not_skip_text() {
        let wt = report(
            "copy",
            vec![ArmSkip::from_grove(
                WorktreeArm::GroveFuse,
                GroveSkip::DaemonDeclined,
            )],
            Some("current"),
        );
        let event = emit_from(
            WorktreeLifecycle::Create,
            CloneOutcome::Success,
            true,
            Some("request"),
            None,
            Some(&wt),
            None,
        );
        assert_eq!(
            event.fallback_reason,
            Some(CloneFallbackReason::DaemonDeclined)
        );
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("http"), "{text}");
        assert!(!text.contains("url"), "{text}");
        assert!(!text.contains("repo"), "{text}");
        assert!(!text.contains("/dev/fuse"), "{text}");
        assert!(!text.contains("declined or unreachable"), "{text}");
        assert!(!text.contains("grove-fuse:"), "{text}");
    }

    #[test]
    fn worktree_ended_remote_kill_is_typed() {
        let event = emit_from(
            WorktreeLifecycle::Create,
            CloneOutcome::Success,
            false,
            Some("remote_kill"),
            None,
            Some(&report("copy", Vec::new(), Some("unknown"))),
            None,
        );
        assert_eq!(event.requested_strategy, Some(CloneStrategy::Grove));
        assert_eq!(event.resolved_strategy, Some(CloneStrategy::Copy));
        assert_eq!(event.fallback_reason, Some(CloneFallbackReason::RemoteKill));
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("remote Grove is off"), "{text}");
    }

    #[test]
    fn worktree_ended_remote_unavailable_is_typed() {
        let event = emit_from(
            WorktreeLifecycle::Create,
            CloneOutcome::Success,
            false,
            Some("remote_unavailable"),
            None,
            Some(&report("copy", Vec::new(), Some("unknown"))),
            None,
        );
        assert_eq!(event.requested_strategy, Some(CloneStrategy::Grove));
        assert_eq!(event.resolved_strategy, Some(CloneStrategy::Copy));
        assert_eq!(
            event.fallback_reason,
            Some(CloneFallbackReason::RemoteUnavailable)
        );
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("remote Grove is unavailable"), "{text}");
    }

    #[test]
    fn worktree_ended_fork_cancel_records_disposition() {
        let event = emit_from(
            WorktreeLifecycle::Fork,
            CloneOutcome::Cancelled,
            true,
            Some("request"),
            None,
            Some(&report("grove-fuse", Vec::new(), Some("current"))),
            Some(CloneCancellationDisposition::ClientCancelled),
        );
        assert_eq!(event.lifecycle, WorktreeLifecycle::Fork);
        assert_eq!(event.outcome, CloneOutcome::Cancelled);
        assert_eq!(
            event.cancellation_disposition,
            Some(CloneCancellationDisposition::ClientCancelled)
        );
    }

    #[test]
    fn worktree_ended_untyped_grove_skip_is_other_and_drops_detail() {
        let wt = report(
            "copy",
            vec![ArmSkip::new(
                WorktreeArm::GroveFuse,
                "/dev/fuse or fusermount missing",
            )],
            Some("unknown"),
        );
        let event = emit_from(
            WorktreeLifecycle::Create,
            CloneOutcome::Success,
            true,
            Some("request"),
            None,
            Some(&wt),
            None,
        );
        assert_eq!(event.fallback_reason, Some(CloneFallbackReason::Other));
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("/dev/fuse"), "{text}");
        assert!(!text.contains("fusermount"), "{text}");
    }

    #[test]
    fn worktree_ended_source_is_grove_rewrite_is_typed() {
        let event = emit_from(
            WorktreeLifecycle::Create,
            CloneOutcome::Success,
            true,
            Some("request"),
            Some(xai_fast_worktree::SKIP_SOURCE_IS_GROVE_MOUNT),
            Some(&report("git", Vec::new(), None)),
            None,
        );
        assert_eq!(
            event.fallback_reason,
            Some(CloneFallbackReason::SourceIsGrove)
        );
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("source is itself"), "{text}");
    }

    #[test]
    fn worktree_ended_source_is_grove_skip_variant_is_typed() {
        let wt = report(
            "copy",
            vec![ArmSkip::from_grove(
                WorktreeArm::GroveFuse,
                GroveSkip::SourceIsGroveMount,
            )],
            None,
        );
        let event = emit_from(
            WorktreeLifecycle::Create,
            CloneOutcome::Success,
            true,
            Some("request"),
            None,
            Some(&wt),
            None,
        );
        assert_eq!(
            event.fallback_reason,
            Some(CloneFallbackReason::SourceIsGrove)
        );
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("source is itself"), "{text}");
        assert!(!text.contains("Grove mount"), "{text}");
        assert!(!text.contains("grove-fuse:"), "{text}");
    }

    #[test]
    fn worktree_ended_failed_without_report_omits_resolved_fields() {
        let event = emit_from(
            WorktreeLifecycle::Create,
            CloneOutcome::Failed,
            true,
            Some("request"),
            None,
            None,
            None,
        );
        assert_eq!(event.requested_strategy, Some(CloneStrategy::Grove));
        assert_eq!(event.resolved_strategy, None);
        assert_eq!(event.transport, None);
        assert_eq!(event.fallback_reason, None);
        assert_eq!(event.daemon_capability_class, None);
        let json = serde_json::to_value(&event).unwrap();
        assert!(json.get("resolved_strategy").is_none());
        assert!(json.get("transport").is_none());
        assert!(json.get("fallback_reason").is_none());
        assert!(json.get("daemon_capability_class").is_none());
        assert!(json.get("cancellation_disposition").is_none());
    }

    #[test]
    fn worktree_ended_builder_in_flight_is_typed() {
        let event = emit_worktree_ended(WorktreeEndedEmit {
            lifecycle: WorktreeLifecycle::Create,
            outcome: CloneOutcome::Failed,
            duration_ms: 4,
            grove_enabled: true,
            grove_gate_source: Some("request"),
            requested_type: WorktreeType::Linked,
            rewrite_reason: None,
            worktree: None,
            cancellation_disposition: None,
            builder_failure: Some(CloneFallbackReason::InFlight),
        });
        assert_eq!(event.fallback_reason, Some(CloneFallbackReason::InFlight));
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("in flight"), "{text}");
        assert!(!text.contains("phase"), "{text}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worktree_ended_fuse_unavailable_maps_the_grove_skip_variant() {
        let wt = report(
            "copy",
            vec![ArmSkip::from_grove(
                WorktreeArm::GroveFuse,
                GroveSkip::FuseUnavailable,
            )],
            Some("unknown"),
        );
        let event = emit_from(
            WorktreeLifecycle::Create,
            CloneOutcome::Success,
            true,
            Some("request"),
            None,
            Some(&wt),
            None,
        );
        assert_eq!(
            event.fallback_reason,
            Some(CloneFallbackReason::FuseUnavailable)
        );
        let text = serde_json::to_string(&event).unwrap();
        assert!(!text.contains("/dev/fuse"), "{text}");
    }
}
