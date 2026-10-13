//! The [`SandboxLaunch`] seam of a [`WorkspaceSandbox`]: what happens to every shell spawn before
//! it runs — the profile, the policy under the live grants, the backend's wrapping — and the
//! record `finish` later consumes. The child's environment is [`wrap_for_mode`]'s alone: this seam
//! reads it for [`bases_from_env`] and never clears, removes or sets a variable.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use xai_grok_sandbox::command::backend::{CommandTag, OriginalArgv, WrapReceipt, wrap_for_mode};
use xai_grok_sandbox::command::grants::Grant;
use xai_grok_sandbox::command::policy::{EnvPolicy, PolicyInputs};
use xai_grok_sandbox::command::violation::bases_from_env;
use xai_grok_sandbox::command::{CallId, CallKind, SandboxMode, SandboxPolicy};
use xai_grok_sandbox::{ProfileName, SandboxProfile, load_sandbox_config};
use xai_grok_telemetry::events::SandboxCommandOutcome;
use xai_grok_tools::sandbox_launch::{LaunchReceipt, SandboxLaunch, SandboxLaunchError, child_env};

use super::calls::CallRecord;
use super::network::{MintedCredential, NetworkSide};
use super::{
    Engaged, WorkspaceSandbox, WorkspaceSandboxError, assert_control_socket_protected, metrics,
};

/// The process-wide profile for the folder (`~/.grok/sandbox.toml` + `.grok/sandbox.toml`,
/// `GROK_SANDBOX_PROFILE` ignored: the per-command policy always starts from `workspace`). A
/// profile that does not resolve is logged and replaced by the bare workspace-writable one.
pub(super) fn resolve_profile(workspace_root: &Path) -> SandboxProfile {
    let config = load_sandbox_config(workspace_root);
    match ProfileName::Workspace.resolve_profile(workspace_root, &config) {
        Ok(profile) => profile,
        Err(error) => {
            tracing::warn!(%error, "sandbox profile did not resolve; using the bare workspace profile");
            SandboxProfile {
                name: "workspace".to_owned(),
                read_only: Vec::new(),
                read_write: vec![workspace_root.to_path_buf()],
                deny: Vec::new(),
                write_deny: Vec::new(),
                default_read: true,
                restrict_network: false,
            }
        }
    }
}

impl WorkspaceSandbox {
    /// The spawn's policy under `grants`. The kernel network policy points at `side`, the
    /// folder's running proxy as this spawn snapshotted it (none: the network is off under
    /// `enforce`, direct under `observe`); the pointers the build writes for that listener are
    /// cleared here, so the policy leaves with none: [`Self::point_at_proxy`] is the one writer
    /// of the pointers, and it writes them together with the credential minted for the spawn on
    /// the same side or not at all.
    pub(super) fn build_policy(
        &self,
        engaged: &Engaged,
        grants: &[Grant],
        side: Option<&Arc<NetworkSide>>,
    ) -> Result<SandboxPolicy, WorkspaceSandboxError> {
        let mut policy = SandboxPolicy::build(PolicyInputs {
            workspace_root: &engaged.root,
            profile: &engaged.profile,
            grants,
            proxy: side.map(|side| side.policy_endpoint()),
            tmp_dirs: engaged.tmp_dirs,
            control_socket_dir: &self.control_socket_dir,
            grok_home: &self.grok_home,
            user_home: self.user_home.as_deref(),
            git_env: &self.git_env,
        })?;
        policy.env.set.clear();
        assert_control_socket_protected(&policy, &self.control_socket_dir)?;
        Ok(policy)
    }

    /// What the spawn's environment says about the proxy: never a pointer without the credential
    /// minted for this spawn, and none at all unless the spawn runs wrapped — `observe` observes
    /// the file system only, so its commands are not pointed at a proxy. No proxy to mint
    /// against leaves no pointer (logged once); a proxy that mints nothing refuses the spawn.
    /// The terminal's own spawns are pointed at nothing.
    fn point_at_proxy(
        &self,
        call: &CallId,
        tag: &CommandTag,
        mode: SandboxMode,
        side: Option<&Arc<NetworkSide>>,
        policy: &mut SandboxPolicy,
    ) -> Result<Option<MintedCredential>, WorkspaceSandboxError> {
        debug_assert!(
            policy.env.set.is_empty(),
            "the policy build leaves no pointer"
        );
        let CallKind::Tool = call.kind() else {
            return Ok(None);
        };
        if !mode.is_wrapped() {
            return Ok(None);
        }
        #[cfg(test)]
        self.run_staged_before_mint_for_test();
        let minted = side
            .map(|side| self.mint_call_credential(side, tag))
            .transpose()?
            .flatten();
        match &minted {
            Some(credential) => policy.env.set = EnvPolicy::proxy_vars_at(&credential.proxy_url()),
            None => tracing::warn!(
                %call,
                "the folder has no egress proxy to mint against; the command runs with no proxy pointer"
            ),
        }
        Ok(minted)
    }

    /// Under `enforce` no command runs while the folder's session directory is not safe: the
    /// floor could not keep a command from planting it.
    fn refuse_unless_session_dir_safe(
        &self,
        mode: SandboxMode,
    ) -> Result<(), WorkspaceSandboxError> {
        if mode != SandboxMode::Enforce {
            return Ok(());
        }
        let Some(reason) = self.session_dir_unsafe() else {
            return Ok(());
        };
        metrics::command(mode, self.backend_name(), SandboxCommandOutcome::WrapFailed);
        tracing::warn!(%reason, "sandbox refused to prepare the command: session directory not safe");
        Err(WorkspaceSandboxError::SessionDirUnsafe { reason })
    }

    fn prepare_inner(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        restored: &[(OsString, OsString)],
        call: &CallId,
    ) -> Result<Option<LaunchReceipt>, WorkspaceSandboxError> {
        // Read, not taken: a retry after a try that got no child runs under what the first try
        // did. The call's mode is the one it holds — pinned at the hub's dispatch, or fixed by its
        // first spawn; a call with neither takes the folder's, once, here, no weaker than its floor
        let (held, floor, settling, session) = {
            let calls = self.calls.lock();
            (
                calls.held_mode(call),
                calls.floor_of(call),
                calls.attempt_settling(call),
                calls.session_of(call),
            )
        };
        let mode = held.unwrap_or_else(|| self.mode().max(floor.unwrap_or_default()));
        if mode == SandboxMode::Off {
            return Ok(None);
        }
        // Switched on since the last async step engaged the folder: `observe` runs unwrapped
        let Some(engaged) = self.engaged.get() else {
            if mode == SandboxMode::Observe {
                return Ok(None);
            }
            metrics::command(mode, None, SandboxCommandOutcome::WrapFailed);
            return Err(WorkspaceSandboxError::NotEngaged);
        };
        self.refuse_unless_session_dir_safe(mode)?;
        // Another session's "for this conversation" rows are not this call's
        let mut grants = self.live_allows_now(session.as_deref());
        // The call-scoped rows apply to this spawn's policy and, through the record, to the
        // connections the proxy decides for it while it runs
        let inherited_once = settling.once;
        grants.extend(inherited_once.iter().cloned());
        // One reading of the folder's proxy for the policy's port and the pointer's credential;
        // a spawn that runs unwrapped has no proxy
        let side = mode
            .is_wrapped()
            .then(|| self.running_network_side())
            .flatten();
        let mut policy = self
            .build_policy(engaged, &grants, side.as_ref())
            .inspect_err(|_| {
                metrics::command(mode, self.backend_name(), SandboxCommandOutcome::WrapFailed);
            })?;
        let extra_bases = bases_from_env(child_env(cmd, restored.iter().cloned()));
        let tag = CommandTag::for_call(call);
        let minted = self
            .point_at_proxy(call, &tag, mode, side.as_ref(), &mut policy)
            .inspect_err(|_| {
                metrics::command(mode, self.backend_name(), SandboxCommandOutcome::WrapFailed);
            })?;
        let backend = engaged.backend.as_deref();
        let receipt: Option<WrapReceipt> =
            match wrap_for_mode(mode, backend, cmd, original, &policy, &tag) {
                Ok(receipt) => receipt,
                Err(error) => {
                    metrics::command(mode, self.backend_name(), SandboxCommandOutcome::WrapFailed);
                    tracing::warn!(%call, %error, "sandbox refused to prepare the command");
                    return Err(if backend.is_none() && mode == SandboxMode::Enforce {
                        WorkspaceSandboxError::EnforceUnavailable
                    } else {
                        WorkspaceSandboxError::Wrap(error)
                    });
                }
            };
        let sandboxed = mode == SandboxMode::Enforce && receipt.is_some();
        let backend_name = receipt
            .as_ref()
            .map(|r| r.backend)
            .or_else(|| self.backend_name());
        if call.kind() == CallKind::Tool {
            let kept = {
                let mut calls = self.calls.lock();
                let kept = calls.remember(
                    call,
                    CallRecord {
                        policy,
                        mode,
                        backend: backend_name,
                        sandboxed,
                        original: original.clone(),
                        extra_bases,
                        replayed_under: settling.replayed_under,
                        inherited_once,
                    },
                );
                if let Ok(evicted) = &kept {
                    self.revoke_released(evicted);
                }
                kept
            };
            match kept {
                Ok(evicted) => self.release_ended_holds(evicted),
                // The command does not run: the credential minted for it goes with the guard
                Err(refused) => return Err(refused.into()),
            }
            // The spawn's record holds the credential now; `finish` or the release revokes it
            if let Some(minted) = minted {
                minted.keep();
            }
        }
        Ok(Some(LaunchReceipt {
            sandboxed,
            backend: backend_name,
        }))
    }
}

impl SandboxLaunch for WorkspaceSandbox {
    fn prepare(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        call: &CallId,
    ) -> Result<Option<LaunchReceipt>, SandboxLaunchError> {
        self.prepare_restoring(cmd, original, &[], call)
    }

    fn prepare_restoring(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        restored: &[(OsString, OsString)],
        call: &CallId,
    ) -> Result<Option<LaunchReceipt>, SandboxLaunchError> {
        self.prepare_inner(cmd, original, restored, call)
            .map_err(SandboxLaunchError::new)
    }

    /// A background child is gone: its record and the credential its `HTTP_PROXY` carried go —
    /// revoked whether or not the hub has the call's start yet, so a grandchild it daemonized
    /// authenticates no longer — and every hold still parked for it is refused.
    fn exited(&self, call_id: &CallId) {
        let (released, enforced) = {
            let mut calls = self.calls.lock();
            let released = calls.exited(call_id);
            self.revoke_released(&released);
            (released, calls.any_runs_enforced())
        };
        self.release_ended_holds(released);
        self.resync_unless_enforced(enforced);
    }
}
