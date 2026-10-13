//! Per-command sandboxing: the policy a single child command runs under, the backend that renders
//! it into an OS wrapper, the decoder that turns a denial into a grantable [`violation::Violation`],
//! and the expiring grant store that widens the next policy.
//!
//! Invariants this module upholds, independent of backend:
//! - The daemon process is never confined; only the spawned command is. Nothing here calls the
//!   process-wide `nono` sandbox.
//! - A wrapper failure is a refusal to run, never a silent unsandboxed launch.
//! - Protected paths ([`policy::SandboxPolicy::protected`]) are rendered last by every backend and
//!   are never grantable; a grant inside one is rejected at load and at `with_grant`.
//! - Expiry is evaluated at every policy build through an injected [`grants::Clock`], never at load.

pub mod backend;
pub mod canonical;
pub mod detect;
pub mod env;
mod git_config;
pub mod grant_store;
pub mod grants;
/// The rollout mode lives in `xai_grok_config::sandbox_mode`, so `RemoteSettings` (which that
/// crate owns) carries it without depending on this crate; `command::mode` re-exports it.
pub mod mode {
    pub use xai_grok_config::sandbox_mode::*;
}
pub mod observe;
pub mod policy;
pub mod protected;
pub mod violation;

#[cfg(target_os = "macos")]
pub use backend::seatbelt::SeatbeltBackend;
pub use backend::{
    BackendCapabilities, BackendChoice, BackendName, CallId, CallKind, CommandTag, HostProbe,
    OriginalArgv, RenderedPolicy, SandboxBackend, SandboxCommandError, WrapReceipt, detect_backend,
    wrap_for_mode,
};
pub use canonical::{RootMoved, ServedRoot, canonical_path, is_same_path};
pub use git_config::{GitConfigEnv, GitMetadataUnread};
pub use grant_store::{GrantError, GrantStore, allows_not_denied, canonical_subject};
pub use grants::{
    Clock, Expiry, FixedClock, Grant, GrantDecision, GrantId, GrantScope, GrantSubject,
    HostPattern, Provenance, SystemClock,
};
pub use mode::{
    ResolvedSandboxMode, SANDBOX_MODE_ENV, SandboxMode, SandboxModeLayers, SandboxModeSource,
};
pub use observe::{
    OBSERVE_SUMMARY_MAX_ROWS, ObserveSummary, WouldBlock, WouldBlockKind, WouldVerdict,
};
pub use policy::{
    BUILD_CACHE_TREES, DenyEntry, EnvPolicy, NetworkPolicy, PolicyError, PolicyInputs,
    ProxyEndpoint, ReadPolicy, SandboxPolicy, VERIFIED_BUILD_CACHES, WritableLocations,
    daemon_tmp_dirs, default_tmp_dirs,
};
pub use protected::Protected;
pub use violation::{
    Blocked, Capability, CommandExit, DecodeInput, Disposition, HoldAnswer, InformationalReason,
    ProposalBounds, RefusedUnderGrant, Replay, Violation, bases_from_env, decode, is_too_broad,
    net_target, refused_under_grant,
};
