//! Host probing and backend selection. [`HostProbe::run`] reads the facts once per daemon start;
//! [`HostProbe::preferred_backend`] is the pure selection the tests pin; [`detect_backend`] maps
//! that choice onto the backend compiled into this binary. `None` always means "no filesystem
//! sandbox on this host": under an `Enforce` the developer chose every command is refused, never
//! run bare; an `Enforce` the rollout switch set runs as `Off` there
//! (`ResolvedSandboxMode::on_host`). The reason for `None` names what was checked and what was
//! found — the OS, the path, what is at it and its mode bits — so `sandbox.status` can answer
//! "why no backend" without a shell on the host.

use std::fmt;
use std::path::Path;

use crate::command::backend::{BackendName, SandboxBackend};

/// The Seatbelt entry point, probed by absolute path: the program `exec` would run.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Which backend a probe result selects, and why not when it selects none. Pure so the table is
/// testable from any platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    Selected(BackendName),
    Unavailable { reason: String },
}

/// Host facts probed once per daemon start. Tests construct it directly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostProbe {
    /// macOS: `/usr/bin/sandbox-exec` exists and applies the trivial profile.
    pub sandbox_exec_ok: bool,
    /// What the probe found at [`SANDBOX_EXEC`], for the reason when it selects no backend
    /// ([`HostProbe::preferred_backend`]).
    pub sandbox_exec: ProbedFile,
}

impl HostProbe {
    /// Probe the running host. On macOS this spawns `/usr/bin/sandbox-exec` once around
    /// `/usr/bin/true` with a bounded wait; elsewhere it spawns nothing.
    pub fn run() -> HostProbe {
        HostProbe {
            sandbox_exec_ok: sandbox_exec_ok_on_host(),
            sandbox_exec: ProbedFile::read(Path::new(SANDBOX_EXEC)),
        }
    }

    /// The backend this host would use for the current target OS: Seatbelt on macOS when
    /// `sandbox-exec` works, nothing anywhere else.
    pub fn preferred_backend(&self) -> BackendChoice {
        if cfg!(target_os = "macos") {
            return self.preferred_macos_backend();
        }
        BackendChoice::Unavailable {
            reason: format!(
                "no per-command sandbox backend for {os}: v1 has Seatbelt on macos only \
                 (checked {SANDBOX_EXEC}: {found})",
                os = std::env::consts::OS,
                found = self.sandbox_exec,
            ),
        }
    }

    /// Seatbelt through `/usr/bin/sandbox-exec` by absolute path, or nothing, with what the probe
    /// found there.
    pub fn preferred_macos_backend(&self) -> BackendChoice {
        if self.sandbox_exec_ok {
            return BackendChoice::Selected(BackendName::Seatbelt);
        }
        let found = match &self.sandbox_exec {
            // Executable, yet the spawned probe of the trivial profile failed
            ProbedFile::File { mode } if is_executable_mode(*mode) => {
                format!("mode {mode:04o}, executable, but it did not apply the trivial profile")
            }
            found => found.to_string(),
        };
        BackendChoice::Unavailable {
            reason: format!("Seatbelt needs {SANDBOX_EXEC}; checked it: {found}"),
        }
    }
}

/// What a probe found at a path it checked, kept for the reason when it selects no backend.
/// Read through a symlink on purpose: the question is about the program `exec` would run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ProbedFile {
    /// Not looked at: a probe a test constructed.
    #[default]
    Unchecked,
    /// `stat` failed; `error` is the OS's text (`No such file or directory (os error 2)`).
    Missing { error: String },
    /// Something other than a regular file (`directory`, `fifo`, `socket`, `device`).
    NotAFile { kind: String },
    /// A regular file with these permission bits (`0o755`); executable when any `x` bit is set.
    File { mode: u32 },
}

impl ProbedFile {
    /// `stat` `path` as `exec` would resolve it.
    pub fn read(path: &Path) -> ProbedFile {
        match std::fs::metadata(path) {
            Err(error) => ProbedFile::Missing {
                error: error.to_string(),
            },
            Ok(meta) if meta.is_file() => ProbedFile::File {
                mode: permission_bits(&meta),
            },
            Ok(meta) => ProbedFile::NotAFile {
                kind: file_kind(&meta.file_type()).to_owned(),
            },
        }
    }

    /// A regular file with an `x` bit.
    pub fn is_executable(&self) -> bool {
        matches!(self, ProbedFile::File { mode } if is_executable_mode(*mode))
    }
}

impl fmt::Display for ProbedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProbedFile::Unchecked => f.write_str("not checked"),
            ProbedFile::Missing { error } => write!(f, "not found ({error})"),
            ProbedFile::NotAFile { kind } => write!(f, "a {kind}, not a regular file"),
            ProbedFile::File { mode } if is_executable_mode(*mode) => {
                write!(f, "mode {mode:04o}, executable")
            }
            ProbedFile::File { mode } => write!(f, "mode {mode:04o}, not executable"),
        }
    }
}

fn is_executable_mode(mode: u32) -> bool {
    mode & 0o111 != 0
}

/// The permission bits of a file (`0o755`); off Unix every file counts as executable.
#[cfg(unix)]
fn permission_bits(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn permission_bits(_meta: &std::fs::Metadata) -> u32 {
    0o755
}

fn file_kind(file_type: &std::fs::FileType) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if file_type.is_fifo() {
            return "fifo";
        }
        if file_type.is_socket() {
            return "socket";
        }
        if file_type.is_char_device() || file_type.is_block_device() {
            return "device";
        }
    }
    if file_type.is_dir() {
        return "directory";
    }
    "special file"
}

/// macOS: `/usr/bin/sandbox-exec` applies the trivial profile (a spawned probe, which fails just
/// the same when the binary is missing or not executable); the binary alone is not enough evidence
/// because a managed host can refuse `sandbox_apply`. [`ProbedFile`] records what is at the path.
#[cfg(target_os = "macos")]
fn sandbox_exec_ok_on_host() -> bool {
    crate::command::backend::seatbelt::probe_sandbox_exec()
}

/// Elsewhere only the file is looked at (through a symlink, as `exec` would); nothing selects
/// Seatbelt off macOS whatever this says.
#[cfg(not(target_os = "macos"))]
fn sandbox_exec_ok_on_host() -> bool {
    ProbedFile::read(Path::new(SANDBOX_EXEC)).is_executable()
}

/// The backend compiled into this binary for the probe's preferred choice, with a log line saying
/// why there is none, so a host with the primitive but without the backend is distinguishable
/// from a host without either. Either way an `enforce` the developer chose refuses every command
/// and one the rollout switch set runs as `off` (`ResolvedSandboxMode::on_host`).
pub fn detect_backend(probe: &HostProbe) -> Option<Box<dyn SandboxBackend>> {
    match probe.preferred_backend() {
        BackendChoice::Selected(name) => {
            let backend = registered_backend(name);
            if backend.is_none() {
                tracing::warn!(
                    backend = <&str>::from(name),
                    "per-command sandbox backend is not compiled into this build; a developer's \
                     enforce refuses every command, the rollout switch's runs off"
                );
            }
            backend
        }
        BackendChoice::Unavailable { reason } => {
            tracing::warn!(
                %reason,
                "no per-command sandbox backend on this host; a developer's enforce refuses every \
                 command, the rollout switch's runs off"
            );
            None
        }
    }
}

fn registered_backend(name: BackendName) -> Option<Box<dyn SandboxBackend>> {
    match name {
        #[cfg(target_os = "macos")]
        BackendName::Seatbelt => Some(Box::new(
            crate::command::backend::seatbelt::SeatbeltBackend::new(),
        )),
        #[cfg(not(target_os = "macos"))]
        BackendName::Seatbelt => None,
    }
}

#[cfg(test)]
#[path = "detect_tests.rs"]
mod tests;
