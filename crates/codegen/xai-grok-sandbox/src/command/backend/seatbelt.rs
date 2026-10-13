//! macOS Seatbelt backend: one command becomes `/usr/bin/sandbox-exec -p <profile> -D K=V … --
//! <program> <args>`. Invariants:
//! - `/usr/bin/sandbox-exec` by absolute path only; a `PATH` lookup could be redirected by the very
//!   command being confined.
//! - `std`/`tokio` `Command` cannot change its program after construction, so `wrap` builds the
//!   wrapper command and swaps it into place, carrying over the original's environment entries
//!   and working directory. Stdio, `pre_exec` hooks and `kill_on_drop` do not survive the swap and
//!   must be applied by the spawn site after `wrap`.

use std::ffi::OsString;
use std::io::{self, Read as _};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::command::backend::seatbelt_profile::{Sbpl, render_enforce};
use crate::command::backend::{
    BackendCapabilities, BackendName, CommandTag, OriginalArgv, RenderedPolicy, SandboxBackend,
    SandboxCommandError, WrapReceipt,
};
pub use crate::command::detect::SANDBOX_EXEC;
#[cfg(doc)]
use crate::command::policy::EnvPolicy;
use crate::command::policy::{PolicyError, SandboxPolicy, symlink_free_spelling};

/// The smallest profile `sandbox-exec` accepts; used only to prove the binary applies profiles.
const TRIVIAL_PROFILE: &str = "(version 1)(allow default)";

/// How long the once-per-start probe waits for `sandbox-exec /usr/bin/true`.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bound on the fallback descriptor sweep when `sysconf(_SC_OPEN_MAX)` is unlimited or fails.
/// Covers the raised soft limit seen on developer Macs (1,048,575) in full; measured cost of the
/// `fcntl` loop: ~2.6 ms at the 10,240 default, ~100 ms at this ceiling.
const FD_SWEEP_CEILING: libc::c_int = 1 << 20;

/// Entries the child-side `proc_pidinfo` listing can hold, on the stack: 32 KiB. A daemon with
/// more descriptors open than this at the fork takes the fallback sweep instead.
const FD_LIST_CAPACITY: usize = 4096;

#[derive(Default)]
pub struct SeatbeltBackend;

impl SeatbeltBackend {
    pub fn new() -> SeatbeltBackend {
        SeatbeltBackend
    }
}

impl SandboxBackend for SeatbeltBackend {
    fn name(&self) -> BackendName {
        BackendName::Seatbelt
    }

    fn capabilities(&self) -> BackendCapabilities {
        // Seatbelt enforces the whole v1 model: writable roots, read denies, protected subpaths,
        // network off / proxy-only.
        BackendCapabilities { reduced: false }
    }

    fn wrap(
        &self,
        cmd: &mut tokio::process::Command,
        original: &OriginalArgv,
        policy: &SandboxPolicy,
        tag: &CommandTag,
    ) -> Result<WrapReceipt, SandboxCommandError> {
        for root in &policy.write_roots {
            refuse_symlinked_root(root)?;
        }
        let sbpl = render_enforce(policy, tag)?;
        install_wrapper(cmd, original, sbpl)
    }
}

/// Builds the `sandbox-exec` argv for `sbpl`, so tests can assert it without a `Command`.
pub(crate) fn sandbox_exec_args(sbpl: &Sbpl, original: &OriginalArgv) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![OsString::from("-p"), OsString::from(&sbpl.profile)];
    for (name, value) in &sbpl.params {
        args.push(OsString::from("-D"));
        args.push(OsString::from(format!("{name}={value}")));
    }
    args.push(OsString::from("--"));
    args.push(original.program.as_os_str().to_owned());
    args.extend(original.args.iter().cloned());
    args
}

/// Swaps the `sandbox-exec` wrapper into `cmd`, detaching nothing: every spawn site registers
/// `detach_command` on the wrapper after `wrap`, and a second `setsid` hook fails the spawn
/// (`EPERM` from a session leader). The environment is carried entry for entry — the
/// [`EnvPolicy`] was applied to `cmd` by [`super::wrap_for_mode`], and this is the only place that
/// reads it. A spawn site must not `env_clear` the command: `Command` does not report a clear, so
/// the wrapper could not reproduce it.
fn install_wrapper(
    cmd: &mut tokio::process::Command,
    original: &OriginalArgv,
    sbpl: Sbpl,
) -> Result<WrapReceipt, SandboxCommandError> {
    let mut wrapper = tokio::process::Command::new(SANDBOX_EXEC);
    wrapper.args(sandbox_exec_args(&sbpl, original));
    for (name, value) in cmd.as_std().get_envs() {
        match value {
            Some(value) => {
                wrapper.env(name, value);
            }
            None => {
                wrapper.env_remove(name);
            }
        }
    }
    wrapper.current_dir(&original.cwd);
    let fallback_end = open_max_bound();
    // SAFETY: the hook calls only `getpid`, `proc_pidinfo` into a stack buffer and `fcntl` on
    // integer descriptors — no allocation, no lock — so it is safe between `fork` and `exec` in
    // a multi-threaded parent.
    unsafe {
        wrapper.pre_exec(move || mark_inherited_fds_cloexec(fallback_end));
    }
    *cmd = wrapper;
    Ok(WrapReceipt {
        backend: BackendName::Seatbelt,
        rendered: RenderedPolicy::Sbpl {
            profile: sbpl.profile,
            params: sbpl.params,
        },
    })
}

/// `sysconf(_SC_OPEN_MAX)` clamped to [`FD_SWEEP_CEILING`], or the ceiling when unavailable:
/// the exclusive end of the fallback sweep, decided in the parent where `sysconf` may allocate.
fn open_max_bound() -> libc::c_int {
    // SAFETY: `sysconf` takes no pointers.
    let open_max = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    libc::c_int::try_from(open_max)
        .ok()
        .filter(|max| *max > 0)
        .map_or(FD_SWEEP_CEILING, |max| max.min(FD_SWEEP_CEILING))
}

/// The child half of the descriptor sweep, run between `fork` and `exec`: lists the descriptors
/// this process has open right now and marks every one from 3 up close-on-exec, so `sandbox-exec`
/// and the command start with only fds 0–2. `std` has already dup'd the stdio pipes into 0–2 by
/// the time the hook runs. When the listing is unavailable, every fd in `3..fallback_end` is
/// visited instead; a closed fd answers `EBADF`, which is ignored.
fn mark_inherited_fds_cloexec(fallback_end: libc::c_int) -> io::Result<()> {
    let mut entries = [libc::proc_fdinfo {
        proc_fd: 0,
        proc_fdtype: 0,
    }; FD_LIST_CAPACITY];
    match list_open_fds(&mut entries) {
        Some(listed) => {
            for entry in listed.iter().filter(|entry| entry.proc_fd >= 3) {
                set_cloexec(entry.proc_fd);
            }
        }
        None => {
            for fd in 3..fallback_end {
                set_cloexec(fd);
            }
        }
    }
    Ok(())
}

fn set_cloexec(fd: libc::c_int) {
    // SAFETY: `fcntl(F_SETFD)` on an integer descriptor takes no pointers and is
    // async-signal-safe; EBADF for an unused fd is the common case.
    unsafe {
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
}

/// The descriptors open in this process per `proc_pidinfo(PROC_PIDLISTFDS)`, written into
/// `entries`; `None` when the call fails or the table may not have fit, so the caller sweeps the
/// whole range instead. Allocation-free: safe to call from a `pre_exec` hook.
pub(crate) fn list_open_fds(entries: &mut [libc::proc_fdinfo]) -> Option<&[libc::proc_fdinfo]> {
    let capacity_bytes = std::mem::size_of_val(entries);
    // SAFETY: `getpid` takes no pointers; the buffer is `entries` and its byte size is passed
    // alongside, so the kernel writes at most that many bytes.
    let written = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDLISTFDS,
            0,
            entries.as_mut_ptr().cast(),
            libc::c_int::try_from(capacity_bytes).ok()?,
        )
    };
    let written = usize::try_from(written).ok()?;
    if written == 0 || written >= capacity_bytes {
        return None;
    }
    entries.get(..written / std::mem::size_of::<libc::proc_fdinfo>())
}

/// A write root is canonical when the policy is built, so a symlink below its top-level component
/// at wrap time means the tree changed in between; the command does not run against a root that
/// no longer names what the policy allowed. The walk is the policy's own
/// ([`symlink_free_spelling`]): the same host condition refuses with the same
/// [`PolicyError`] whether a grant or a wrap meets it. The spelled workspace root itself is the
/// policy build's to check, before it is canonicalised; this cannot see the spelling.
fn refuse_symlinked_root(root: &Path) -> Result<(), SandboxCommandError> {
    if !root.is_absolute() {
        return Err(PolicyError::NotAbsolute {
            path: root.to_path_buf(),
        }
        .into());
    }
    symlink_free_spelling(root)?;
    Ok(())
}

/// Spawns the trivial profile around `/usr/bin/true`; false when `sandbox-exec` cannot be spawned,
/// refuses the profile, or does not exit within [`PROBE_TIMEOUT`]. The `HostProbe` macOS arm
/// records this as `sandbox_exec_ok`; what the probe found is logged here, once per daemon start,
/// with the child's exit status and stderr (a managed host refusing `sandbox_apply` says why there),
/// so a host whose every `enforce` command is refused has a diagnostic.
pub fn probe_sandbox_exec() -> bool {
    match run_probe(sandbox_exec_probe(), |child| {
        xai_tty_utils::global_process_scope().enroll_std(child)
    }) {
        Ok(()) => true,
        Err(failure) => {
            tracing::warn!(
                path = SANDBOX_EXEC,
                %failure,
                "sandbox-exec did not apply the trivial profile; no per-command sandbox backend on this host"
            );
            false
        }
    }
}

/// `sandbox-exec -p <trivial profile> /usr/bin/true`, stdin and stdout on the shared null
/// descriptor ([`xai_tty_utils::null_stdio`]: `Stdio::null()` opens `/dev/null` by path at spawn
/// time, and an unlinked device would read as a broken `sandbox-exec`), stderr captured.
fn sandbox_exec_probe() -> std::process::Command {
    let mut cmd = std::process::Command::new(SANDBOX_EXEC);
    cmd.args(["-p", TRIVIAL_PROFILE, "/usr/bin/true"])
        .stdin(xai_tty_utils::null_stdio())
        .stdout(xai_tty_utils::null_stdio())
        .stderr(Stdio::piped());
    cmd
}

/// Why the probe did not pass: the verdict for the log line and for the tests.
#[derive(Debug)]
enum ProbeFailure {
    /// `spawn` itself failed: no binary, not executable, or no descriptors left.
    Spawn(io::Error),
    /// The child exited without success; `stderr` is what it said, if anything.
    Exit { status: ExitStatus, stderr: String },
    /// The child was still running at [`PROBE_TIMEOUT`] and was killed.
    Timeout { stderr: String },
}

impl std::fmt::Display for ProbeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeFailure::Spawn(error) => write!(f, "spawn failed: {error}"),
            ProbeFailure::Exit { status, stderr } => {
                write!(f, "exited with {status}")?;
                if !stderr.is_empty() {
                    write!(f, "; stderr: {stderr}")?;
                }
                Ok(())
            }
            ProbeFailure::Timeout { stderr } => {
                write!(f, "did not exit within {PROBE_TIMEOUT:?}, killed")?;
                if !stderr.is_empty() {
                    write!(f, "; stderr: {stderr}")?;
                }
                Ok(())
            }
        }
    }
}

/// How much of the probe's stderr is kept for the log line.
const PROBE_STDERR_LIMIT: u64 = 4096;

/// Runs `cmd` as the probe with the process-scope enrolment injected. The verdict is the child's
/// exit alone: `/usr/bin/true` may be gone before it is enrolled (`ESRCH`), which says nothing
/// about `sandbox-exec`. Stderr is read after the exit or the kill, bounded, whitespace-trimmed.
fn run_probe<G, E: std::fmt::Display>(
    mut cmd: std::process::Command,
    enroll: impl FnOnce(&std::process::Child) -> Result<G, E>,
) -> Result<(), ProbeFailure> {
    xai_tty_utils::detach_std_command(&mut cmd);
    #[allow(clippy::disallowed_methods)] // enrolled via ProcessScope::enroll_std below
    let mut child = cmd.spawn().map_err(ProbeFailure::Spawn)?;
    let _group = enroll(&child)
        .inspect_err(|error| tracing::debug!(%error, "sandbox-exec probe not enrolled"))
        .ok();
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stderr = child
        .stderr
        .take()
        .map(|pipe| {
            let mut text = String::new();
            let _ = pipe.take(PROBE_STDERR_LIMIT).read_to_string(&mut text);
            text.trim().to_owned()
        })
        .unwrap_or_default();
    match status {
        Some(status) if status.success() => Ok(()),
        Some(status) => Err(ProbeFailure::Exit { status, stderr }),
        None => Err(ProbeFailure::Timeout { stderr }),
    }
}

#[cfg(test)]
#[path = "seatbelt_tests.rs"]
mod tests;
