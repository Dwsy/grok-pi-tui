//! The per-command sandbox seam every shell spawn site goes through. The tools crate is
//! policy-blind: it hands the [`tokio::process::Command`] to the injected [`SandboxLaunch`] and
//! spawns whatever comes back; without an implementation the command runs unchanged.
//!
//! The wrapper replaces `*cmd` keeping only program, args, current dir and envs, so every site
//! sets stdio, `pre_exec` hooks, fd mappings, `kill_on_drop` and process-group settings *after*
//! [`prepare`], through [`wire_prepared`]:
//!
//! ```text
//! Command::new(program) → args → current_dir → envs
//!   → sandbox_launch::prepare(hook, &mut cmd, &call)
//!   → fd_mappings (if any)
//!   → sandbox_launch::wire_prepared: stdio → kill_on_drop → detach → restrict_child_network
//!   → spawn()
//! ```

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

pub use xai_grok_sandbox::command::BackendName;
pub use xai_grok_sandbox::command::OriginalArgv;
pub use xai_grok_sandbox::command::{CallId, CallKind};

/// Why the hook refused to let the command run: the daemon's own error, carried across the seam
/// as one typed value so every spawn site maps it once. `Display` is the
/// text the tool reports; `source` is the daemon's error for a host that wants to match on it.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct SandboxLaunchError(Box<dyn std::error::Error + Send + Sync + 'static>);

impl SandboxLaunchError {
    pub fn new(error: impl std::error::Error + Send + Sync + 'static) -> SandboxLaunchError {
        SandboxLaunchError(Box::new(error))
    }

    /// The daemon's error, for a host that knows its type.
    pub fn downcast_ref<E: std::error::Error + 'static>(&self) -> Option<&E> {
        self.0.downcast_ref()
    }
}

/// The one error every spawn site returns for a refusal: the command never ran, and the tool
/// result says why.
impl From<SandboxLaunchError> for std::io::Error {
    fn from(reason: SandboxLaunchError) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("sandbox refused to run the command: {reason}"),
        )
    }
}

impl From<SandboxLaunchError> for crate::computer::types::ComputerError {
    fn from(reason: SandboxLaunchError) -> crate::computer::types::ComputerError {
        std::io::Error::from(reason).into()
    }
}

/// What the hook did to one command. The daemon keeps the full receipt (rendered policy, exact
/// channel) keyed by `call_id`; the tools crate only needs to know whether the child ran wrapped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchReceipt {
    /// True when the command was rewritten to run inside an OS sandbox (`enforce`).
    pub sandboxed: bool,
    pub backend: Option<BackendName>,
}

/// Injected by the daemon. `prepare` rewrites `cmd` in place (or leaves it alone) and must not
/// spawn. An `Err` means the command must not run: the spawn site returns it as the tool error.
/// `call` names the spawn: a tool call the hub dispatched, or one of the terminal backend's own
/// shell-init spawns ([`CallKind::ShellInit`]) that no decoder runs on.
pub trait SandboxLaunch: Send + Sync {
    /// # Errors
    /// The reason the command must not run (no backend under `enforce`, an unrenderable policy).
    fn prepare(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        call: &CallId,
    ) -> Result<Option<LaunchReceipt>, SandboxLaunchError>;

    /// [`SandboxLaunch::prepare`] for a shell that exports variables of its own before the command
    /// runs (the persistent shell replaying its snapshot): `restored` are those variables, part of
    /// the environment the command runs under though `cmd` does not carry them ([`child_env`]).
    /// An implementation that reads no environment keeps the default.
    ///
    /// # Errors
    /// As [`SandboxLaunch::prepare`].
    fn prepare_restoring(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        _restored: &[(OsString, OsString)],
        call: &CallId,
    ) -> Result<Option<LaunchReceipt>, SandboxLaunchError> {
        self.prepare(cmd, original, call)
    }

    /// The backgrounded spawn of `call` exited (or was killed with its terminal). Called once
    /// per background task, possibly before the call's result reaches the hub.
    fn exited(&self, _call: &CallId) {}
}

/// The hook as stored in `Resources` (`resources.get::<SandboxLaunchHook>()`); the terminal
/// backend receives the same `Arc` at construction.
#[derive(Clone)]
pub struct SandboxLaunchHook(Arc<dyn SandboxLaunch>);

impl SandboxLaunchHook {
    pub fn new(launch: Arc<dyn SandboxLaunch>) -> Self {
        Self(launch)
    }

    /// The implementation, for a site that calls [`prepare`] directly.
    pub fn as_launch(&self) -> &dyn SandboxLaunch {
        self.0.as_ref()
    }

    /// The same implementation, shared with a backend constructed for the session.
    pub fn shared(&self) -> Arc<dyn SandboxLaunch> {
        Arc::clone(&self.0)
    }
}

impl std::fmt::Debug for SandboxLaunchHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SandboxLaunchHook(..)")
    }
}

/// What the site is about to run, read back from the pre-wrap command. A command with no
/// `current_dir` runs in the process cwd, which is what the decoder joins relative paths to.
pub fn original_argv(cmd: &tokio::process::Command) -> OriginalArgv {
    let std_cmd = cmd.as_std();
    OriginalArgv {
        program: PathBuf::from(std_cmd.get_program()),
        args: std_cmd.get_args().map(OsString::from).collect(),
        cwd: std_cmd.get_current_dir().map_or_else(
            || std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            std::path::Path::to_path_buf,
        ),
    }
}

/// Run the hook when one is injected. Call it right after program/args/cwd/env are set and before
/// stdio, fd mappings, `pre_exec` and `kill_on_drop` (see the module docs).
///
/// # Errors
/// Whatever the hook refuses with; the site must not spawn.
pub fn prepare(
    hook: Option<&dyn SandboxLaunch>,
    cmd: &mut tokio::process::Command,
    call: &CallId,
) -> Result<Option<LaunchReceipt>, SandboxLaunchError> {
    let Some(hook) = hook else {
        return Ok(None);
    };
    let original = original_argv(cmd);
    hook.prepare(cmd, &original, call)
}

/// [`prepare`] for a shell that exports `restored()` before the command runs
/// ([`SandboxLaunch::prepare_restoring`]); `restored` is read only when a hook is injected.
///
/// # Errors
/// Whatever the hook refuses with; the site must not spawn.
pub fn prepare_restoring(
    hook: Option<&dyn SandboxLaunch>,
    cmd: &mut tokio::process::Command,
    restored: impl FnOnce() -> Vec<(OsString, OsString)>,
    call: &CallId,
) -> Result<Option<LaunchReceipt>, SandboxLaunchError> {
    let Some(hook) = hook else {
        return Ok(None);
    };
    let original = original_argv(cmd);
    hook.prepare_restoring(cmd, &original, &restored(), call)
}

/// The environment the command runs under: what `cmd` spawns with (the daemon's own environment
/// with `cmd`'s sets and removals applied), then `restored` on top. Exact because no spawn site
/// clears `cmd`'s environment: a clear is not reported, so a policy base env removes names instead.
pub fn child_env(
    cmd: &tokio::process::Command,
    restored: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    let mut env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    for (name, value) in cmd.as_std().get_envs() {
        match value {
            Some(value) => env.insert(name.to_os_string(), value.to_os_string()),
            None => env.remove(name),
        };
    }
    env.extend(restored);
    env.into_iter().collect()
}

/// What every spawn site sets on the command [`prepare`] handed back, the wrapper when there is
/// one: stdin closed, stdout piped, `stderr`, `kill_on_drop`, the detach hook, then the
/// child-network filter. A site's fd mappings go on before this, so they map first in the child.
pub fn wire_prepared(cmd: &mut tokio::process::Command, stderr: std::process::Stdio) {
    // No `process_group(0)`: std runs setpgid() before the pre_exec hooks, so the detach hook's
    // setsid() would fail with EPERM
    cmd.stdin(xai_tty_utils::null_stdio())
        .stdout(std::process::Stdio::piped())
        .stderr(stderr)
        .kill_on_drop(true);
    xai_tty_utils::detach_command(cmd);
    xai_grok_sandbox::child_net::restrict_child_network(cmd);
}

/// Tell the hook, when one is injected, that the backgrounded spawn of `call` exited.
pub fn exited(hook: Option<&dyn SandboxLaunch>, call: &CallId) {
    if let Some(hook) = hook {
        hook.exited(call);
    }
}

#[cfg(test)]
#[path = "sandbox_launch_tests.rs"]
mod tests;
