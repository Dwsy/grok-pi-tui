//! File acceleration for local sessions: the gate built from the feature and the process's sandbox state, the
//! session's file system choice, and the accelerator's notices as telemetry events.
//!
//! A session is accelerated only on a positive signal that nothing confines this process, because an accelerator
//! could read files the sandbox denies. An ACP session is never offered to the accelerator: the client owns its files.

use std::sync::Arc;

use tokio::runtime::Handle;
use xai_grok_config_types::{
    ENV_FILE_ACCELERATION_ROUTES, Feature, resolve_file_acceleration_routes,
};
use xai_grok_telemetry::events::{
    FileAccelerationArm, FileAccelerationSessionEnded, FileAccelerationSessionStarted,
    FileAccelerationUnavailableHit,
};
use xai_grok_tools::computer::{
    file_acceleration::{
        self, AcceleratedFs, AccelerationArm, AccelerationContext, AccelerationNotice,
        FileAccelerationGate, LATENCY_BUCKET_EDGES_MS,
    },
    types::AsyncFileSystem,
};

use crate::agent::config::Config;

/// The `file_acceleration` feature and route override a session spawns with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FileAccelerationSettings {
    pub(crate) is_enabled: bool,
    pub(crate) routes: Option<Arc<str>>,
}

pub(crate) fn settings(config: &Config) -> FileAccelerationSettings {
    FileAccelerationSettings {
        is_enabled: config.is_feature_enabled(Feature::FileAcceleration),
        routes: resolve_file_acceleration_routes(
            std::env::var(ENV_FILE_ACCELERATION_ROUTES).ok().as_deref(),
            config.file_acceleration.routes.as_deref(),
            config
                .remote_settings
                .as_ref()
                .and_then(|remote| remote.file_acceleration_routes.as_deref()),
        ),
    }
}

/// The session's file system and the owner of its accelerator summary. `acp_fs` is the client-backed file system
/// when the client serves files; `None` means the local disk. Only the local disk reaches the accelerator, and
/// `gate` is not built at all for an ACP session.
pub(crate) fn select_session_fs(
    acp_fs: Option<Arc<dyn AsyncFileSystem>>,
    gate: impl FnOnce() -> FileAccelerationGate,
) -> AcceleratedFs {
    match acp_fs {
        Some(fs) => AcceleratedFs { fs, session: None },
        None => file_acceleration::local_fs(gate()),
    }
}

/// The gate for one local session, reading the process's sandbox state.
pub(crate) fn gate(settings: FileAccelerationSettings, session_id: &str) -> FileAccelerationGate {
    let decision = decide_gate(GateInputs {
        is_feature_enabled: settings.is_enabled,
        configured_profile: xai_grok_sandbox::configured_profile_name(),
        requested_confinement: xai_grok_sandbox::requested_confinement_profile(),
        is_inside_bwrap: xai_grok_sandbox::is_inside_bwrap(),
    });
    match decision {
        GateDecision::Enabled => {
            let session_id = session_id.to_owned();
            let runtime = Handle::try_current().ok();
            FileAccelerationGate::Enabled(AccelerationContext {
                routes: settings.routes,
                sink: Arc::new(move |notice| {
                    // Telemetry drops an event emitted off a runtime, and an accelerator may
                    // notify from its own thread.
                    let _entered = match (Handle::try_current(), &runtime) {
                        (Err(_), Some(runtime)) => Some(runtime.enter()),
                        (Ok(_), _) | (Err(_), None) => None,
                    };
                    log_notice(&session_id, notice);
                }),
            })
        }
        GateDecision::FeatureOff => FileAccelerationGate::Disabled,
        GateDecision::SandboxUnknown
        | GateDecision::SandboxRequested
        | GateDecision::InsideBwrap => {
            tracing::debug!(reason = ?decision, "file acceleration refused for this session");
            FileAccelerationGate::Disabled
        }
    }
}

/// What the gate reads, passed in so [`decide_gate`] depends on nothing else.
#[derive(Debug, Clone, Copy)]
struct GateInputs<'a> {
    is_feature_enabled: bool,
    /// The profile recorded at startup, `off` included; `None` when startup recorded none.
    configured_profile: Option<&'a str>,
    /// The confining profile this process was started with, if any.
    requested_confinement: Option<&'a str>,
    /// This process is the child of a sandbox's bwrap re-exec.
    is_inside_bwrap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateDecision {
    Enabled,
    FeatureOff,
    /// No profile was recorded, so there is no positive signal that nothing confines this process.
    SandboxUnknown,
    SandboxRequested,
    InsideBwrap,
}

fn decide_gate(inputs: GateInputs<'_>) -> GateDecision {
    if !inputs.is_feature_enabled {
        return GateDecision::FeatureOff;
    }
    if inputs.is_inside_bwrap {
        return GateDecision::InsideBwrap;
    }
    match (inputs.configured_profile, inputs.requested_confinement) {
        (None, _) => GateDecision::SandboxUnknown,
        (Some(_), Some(_)) => GateDecision::SandboxRequested,
        (Some(_), None) => GateDecision::Enabled,
    }
}

/// One notice as the event it is logged as.
#[derive(Debug, PartialEq, Eq)]
enum AccelerationEvent {
    Started(FileAccelerationSessionStarted),
    Ended(FileAccelerationSessionEnded),
    UnavailableHit(FileAccelerationUnavailableHit),
}

fn event_for(session_id: &str, notice: AccelerationNotice) -> AccelerationEvent {
    let session_id = session_id.to_owned();
    match notice {
        AccelerationNotice::SessionStarted { arm } => {
            AccelerationEvent::Started(FileAccelerationSessionStarted {
                session_id,
                arm: telemetry_arm(arm),
            })
        }
        AccelerationNotice::SessionEnded {
            arm,
            duration_ms,
            served,
            declined,
            failed,
            served_latency,
            saturated,
        } => AccelerationEvent::Ended(FileAccelerationSessionEnded {
            session_id,
            arm: telemetry_arm(arm),
            duration_ms,
            served,
            declined,
            failed,
            served_latency_buckets: served_latency.to_vec(),
            served_latency_bucket_edges_ms: LATENCY_BUCKET_EDGES_MS.to_vec(),
            saturated,
        }),
        AccelerationNotice::UnavailableHit { label, retry_in_ms } => {
            AccelerationEvent::UnavailableHit(FileAccelerationUnavailableHit {
                session_id,
                label,
                retry_in_ms,
            })
        }
    }
}

fn telemetry_arm(arm: AccelerationArm) -> FileAccelerationArm {
    match arm {
        AccelerationArm::Treatment => FileAccelerationArm::Treatment,
        AccelerationArm::Control => FileAccelerationArm::Control,
    }
}

/// The accelerator's sink; the tools crate cannot link telemetry.
fn log_notice(session_id: &str, notice: AccelerationNotice) {
    use xai_grok_telemetry::session_ctx::log_event;
    match event_for(session_id, notice) {
        AccelerationEvent::Started(event) => log_event(event),
        AccelerationEvent::Ended(event) => log_event(event),
        AccelerationEvent::UnavailableHit(event) => log_event(event),
    }
}

#[cfg(test)]
#[path = "file_acceleration_tests.rs"]
mod tests;
