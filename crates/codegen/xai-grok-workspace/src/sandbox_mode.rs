//! Per-workspace resolution of the command-sandbox rollout mode: `GROK_SANDBOX_MODE`, then the
//! fleet layer (`RemoteSettings.sandbox_mode`, else the host's `<grok_home>/managed_config.toml`),
//! then `<grok_home>/workspaced.toml`, then the default (`off`); the workspace's
//! `<workspace>/.grok/workspaced.toml` may only tighten the result. On a host with no sandbox
//! backend, an `enforce` the rollout switch set runs as `off` (`SandboxMode::resolve_on_host`);
//! a developer's own `enforce` keeps refusing there.
//!
//! Both files are the daemon's own: the `grok` CLI's `config.toml` is neither read nor written
//! here. They are read as the daemon reads any file a command could have planted — opened
//! without following a symlink or blocking, only when a regular file (the user's own, for the
//! user layer), up to a bound. A symlink on the way to a layer below the folder, or at or below
//! the grok home — the file, a planted `.grok`, a linked grok home — is followed only to a file
//! outside every place a command may write or may have written, and that file is read the same
//! way; `sandbox.mode.set` never writes through one. A layer whose directory is not one (a
//! `.grok` that is a file) is absent, as a missing one is.
//!
//! Invariants:
//!
//! 1. **Never fail open.** A layer that is there but refused or unreadable counts as `enforce`,
//!    the strictest mode, with one warning, and `sandbox.status` shows `degraded:
//!    config_refused`; never as absent, the default `off`. It is marked only when it raised the
//!    mode: beside a readable layer that names `enforce` that layer decided, so a refusal never
//!    ties or lowers it. On a host with no backend nothing enforces, and the readable layers'
//!    mode stands, still marked.
//! 2. **Dotfile links still work.** A symlink whose final target lies outside every place a
//!    command may write, or may have written (`WritableLocations`), is followed, and the target
//!    is read as the file itself would be.
//! 3. **Links into writable places are refused**, as invariant 1 says: into the workspace, a
//!    build-cache tree, or a place a grant or another served folder opened; a link at the file
//!    or at a directory on the way (`.grok`) alike, and `sandbox.mode.set` writes through none.
//!
//! The precedence itself is `xai_grok_sandbox::command::SandboxMode::resolve`; this module reads
//! the layers, so the pure logic and the IO are tested apart.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use xai_grok_config_types::{RemoteSettings, SandboxSettings, WORKSPACED_CONFIG_FILENAME};
use xai_grok_sandbox::command::mode::{
    RefusedLayers, ResolvedSandboxMode, SANDBOX_MODE_ENV, SandboxModeLayers,
};
use xai_grok_sandbox::command::protected::{FileOwner, HeldDir};
use xai_grok_sandbox::command::{SandboxMode, WritableLocations, canonical_path, is_same_path};

/// Every input of [`resolve_sandbox_mode_in`], injected so tests never touch the process
/// environment or the user's grok home.
#[derive(Clone, Copy, Debug)]
pub struct SandboxModeInputs<'a> {
    pub workspace_root: &'a Path,
    pub grok_home: &'a Path,
    /// The already-read `GROK_SANDBOX_MODE` value.
    pub env: Option<&'a str>,
    /// The fleet's remote settings, when the caller has them; `None` reads the host's managed
    /// config file for them ([`managed_mode_layer`]).
    pub remote: Option<&'a RemoteSettings>,
    /// Whether this host has a sandbox backend (`detect_backend` selected one): the input of
    /// [`SandboxMode::resolve_on_host`], fixed for the daemon's lifetime.
    pub backend_available: bool,
    /// Where a command may write or may have written: a symlinked layer that resolves into it is
    /// refused. `WritableLocations::daemon()` in production.
    pub writable: &'a WritableLocations,
}

/// Resolve the mode for `workspace_root` from the live environment and the user's grok home, on
/// a host with (`backend_available`) or without a sandbox backend.
pub fn resolve_sandbox_mode(
    workspace_root: &Path,
    remote: Option<&RemoteSettings>,
    backend_available: bool,
) -> ResolvedSandboxMode {
    let env = std::env::var(SANDBOX_MODE_ENV).ok();
    resolve_sandbox_mode_in(SandboxModeInputs {
        workspace_root,
        grok_home: &xai_grok_config::grok_home(),
        env: env.as_deref(),
        remote,
        backend_available,
        writable: &WritableLocations::daemon(),
    })
}

/// [`resolve_sandbox_mode`] with every input injected. The fleet layer is the explicit
/// `remote` when the caller has one, else the host's managed config file
/// ([`managed_mode_layer`]), read and refused like the two `workspaced.toml` layers.
pub fn resolve_sandbox_mode_in(inputs: SandboxModeInputs<'_>) -> ResolvedSandboxMode {
    let remote = match inputs.remote {
        Some(remote) => Ok(remote.sandbox_mode),
        None => managed_mode_layer(&inputs),
    };
    let workspace = read_mode_layer(
        &workspace_config_path(inputs.workspace_root),
        inputs.workspace_root,
        FileOwner::Any,
        &inputs,
    );
    let user = read_mode_layer(
        &user_config_path(inputs.grok_home),
        // From the grok home's parent, not `/`: the grok home itself is judged, while a link
        // above it (`/home -> /data/home`) is the host's own layout, which no command may plant
        inputs.grok_home.parent().unwrap_or(inputs.grok_home),
        FileOwner::Daemon,
        &inputs,
    );
    let layers = SandboxModeLayers {
        env: inputs.env,
        remote: remote.as_ref().ok().copied().flatten(),
        user: user.as_ref().ok().copied().flatten(),
        workspace: workspace.as_ref().ok().copied().flatten(),
        refused: RefusedLayers {
            remote: remote.is_err(),
            user: user.is_err(),
            workspace: workspace.is_err(),
        },
    };
    SandboxMode::resolve_on_host(layers, inputs.backend_available)
}

/// The user layer's file: `<grok_home>/workspaced.toml`.
pub fn user_config_path(grok_home: &Path) -> PathBuf {
    grok_home.join(WORKSPACED_CONFIG_FILENAME)
}

/// The workspace layer's file: `<workspace_root>/.grok/workspaced.toml`.
pub fn workspace_config_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".grok")
        .join(WORKSPACED_CONFIG_FILENAME)
}

/// The managed layer's file: `<grok_home>/managed_config.toml`, the organisation's config (the
/// file the config crate's `load_managed_config` reads), under the grok home the user layer is
/// read from. A floor entry: no command writes it.
pub fn managed_config_path(grok_home: &Path) -> PathBuf {
    grok_home.join(xai_grok_config::MANAGED_CONFIG_FILENAME)
}

/// The fleet layer as this host's managed config has it: its `[sandbox] mode`, read when the
/// caller has no [`SandboxModeInputs::remote`]. The daemon fetches no settings from a server, so
/// this file is the one fleet-wide input it has; it is read as the two `workspaced.toml` layers
/// are read — the same held-directory reader from the grok home's parent down, the same
/// `[sandbox]` table, the same handling of a value that is not a mode, the same refusal — rather
/// than through the config crate's whole-document loader, so the three layers cannot drift
/// apart. It is the daemon's own file ([`FileOwner::Daemon`]); a symlink on the way is followed
/// only to a file outside every place a command may write ([`read_layer_text`]). `Ok(None)` when
/// the file is absent or names no mode; `Err` when it is there but refused or unreadable, which
/// the precedence counts as `enforce` ([`RefusedLayers::remote`]) — `off` on a host with no
/// backend — never as absent.
///
/// # Errors
/// [`read_mode_layer`]'s: the file is there but refused or unreadable, not TOML included.
pub fn managed_mode_layer(
    inputs: &SandboxModeInputs<'_>,
) -> Result<Option<SandboxMode>, std::io::Error> {
    read_mode_layer(
        &managed_config_path(inputs.grok_home),
        inputs.grok_home.parent().unwrap_or(inputs.grok_home),
        FileOwner::Daemon,
        inputs,
    )
}

/// Why `sandbox.mode.set` could not write the workspace file (typed, so the
/// daemon's verb maps it once). `Display` is the text the desktop shows.
#[derive(Debug, thiserror::Error)]
pub enum SandboxModeWriteError {
    #[error("cannot read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} is not valid TOML: {source}", path.display())]
    NotToml {
        path: PathBuf,
        #[source]
        source: toml_edit::TomlError,
    },
    #[error("[sandbox] in {} is not a table", path.display())]
    SandboxNotATable { path: PathBuf },
    #[error("cannot create {}: {source}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "{} is the user-wide settings file, not a folder's; set the mode on the folder itself",
        path.display()
    )]
    UserLayer { path: PathBuf },
    #[error("{} is not a directory", path.display())]
    NotADirectory { path: PathBuf },
}

/// `sandbox.mode.set` on one folder: `mode` is written into its workspace layer and the layers
/// are resolved again, so the answer is the mode the folder is in *now* and where it came from
/// (the workspace layer only tightens: a `mode.set off` under a user layer of `observe` answers
/// `observe` / `user_config`). A `workspace_root` that is not a directory is refused — the write
/// creates `.grok/`, never the folder. A served folder's sandbox wraps this and drops its cached
/// resolution; [`set_workspace_mode_at`] is the same for a folder nothing serves.
///
/// # Errors
/// [`SandboxModeWriteError::NotADirectory`], or the writer's.
pub fn set_workspace_mode_in(
    mode: SandboxMode,
    inputs: SandboxModeInputs<'_>,
) -> Result<(PathBuf, ResolvedSandboxMode), SandboxModeWriteError> {
    if !inputs.workspace_root.is_dir() {
        return Err(SandboxModeWriteError::NotADirectory {
            path: inputs.workspace_root.to_path_buf(),
        });
    }
    let path = write_workspace_sandbox_mode_in(inputs.workspace_root, inputs.grok_home, mode)?;
    Ok((path, resolve_sandbox_mode_in(inputs)))
}

/// [`set_workspace_mode_in`] for a folder nothing serves: the live environment, the host's
/// managed config file and the user's grok home — the layers a served folder reads by default —
/// and the daemon's one answer to whether this host has a backend.
///
/// # Errors
/// As [`set_workspace_mode_in`].
pub fn set_workspace_mode_at(
    workspace_root: &Path,
    mode: SandboxMode,
    backend_available: bool,
) -> Result<(PathBuf, ResolvedSandboxMode), SandboxModeWriteError> {
    let env = std::env::var(SANDBOX_MODE_ENV).ok();
    set_workspace_mode_in(
        mode,
        SandboxModeInputs {
            workspace_root,
            grok_home: &xai_grok_config::grok_home(),
            env: env.as_deref(),
            remote: None,
            backend_available,
            writable: &WritableLocations::daemon(),
        },
    )
}

/// Set `[sandbox] mode` in the workspace's `.grok/workspaced.toml` (`sandbox.mode.set`), keeping
/// every other key and comment and replacing the file atomically. Creates `.grok/` and the file
/// when absent. Returns the path written. The CLI's `config.toml` beside it is never touched.
///
/// # Errors
/// The folder's `.grok` is the grok home (the file is the user layer every folder reads) or a
/// symlink ([`HeldDir`]), the file exists but is not TOML, `[sandbox]` is not a table, or the
/// write fails.
pub fn write_workspace_sandbox_mode(
    workspace_root: &Path,
    mode: SandboxMode,
) -> Result<PathBuf, SandboxModeWriteError> {
    write_workspace_sandbox_mode_in(workspace_root, &xai_grok_config::grok_home(), mode)
}

/// [`write_workspace_sandbox_mode`] against an injected grok home.
///
/// # Errors
/// As [`write_workspace_sandbox_mode`].
pub fn write_workspace_sandbox_mode_in(
    workspace_root: &Path,
    grok_home: &Path,
    mode: SandboxMode,
) -> Result<PathBuf, SandboxModeWriteError> {
    let path = workspace_config_path(workspace_root);
    // SECURITY: for the home folder the workspace file is the user layer; a folder's write may
    // only tighten that folder, never switch every folder's mode
    if is_same_path(
        &canonical_path(&workspace_root.join(".grok")),
        &canonical_path(grok_home),
    ) {
        return Err(SandboxModeWriteError::UserLayer { path });
    }
    #[cfg(test)]
    AFTER_USER_LAYER_CHECK.with_borrow_mut(|hook| hook.as_mut().map(|hook| hook()));
    let anchor = canonical_path(workspace_root);
    let create_dir = |source| SandboxModeWriteError::CreateDir {
        path: workspace_root.join(".grok"),
        source,
    };
    let dir =
        HeldDir::open(&anchor, &anchor.join(".grok"), FileOwner::Any, true).map_err(create_dir)?;
    // The root may have been relinked since the check above; the held handle cannot be
    if dir.is(grok_home).map_err(create_dir)? {
        return Err(SandboxModeWriteError::UserLayer { path });
    }
    // Writers of one folder's file take turns: `replace` takes another writer's rename landing
    // between its own and its check for a swap, and removes that file
    #[cfg(unix)]
    dir.lock(MODE_LOCK_WAIT)
        .map_err(|source| SandboxModeWriteError::Write {
            path: path.clone(),
            source,
        })?;
    let name = OsStr::new(WORKSPACED_CONFIG_FILENAME);
    let existing = match dir.read(name, MAX_WORKSPACED_TOML_BYTES, FileOwner::Any) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(SandboxModeWriteError::Read { path, source });
        }
    };
    let mut document: toml_edit::DocumentMut =
        existing
            .parse()
            .map_err(|source| SandboxModeWriteError::NotToml {
                path: path.clone(),
                source,
            })?;
    let sandbox = document.entry("sandbox").or_insert(toml_edit::table());
    let table = sandbox
        .as_table_like_mut()
        .ok_or_else(|| SandboxModeWriteError::SandboxNotATable { path: path.clone() })?;
    table.insert("mode", toml_edit::value(<&str>::from(mode)));
    dir.replace(name, &document.to_string(), FileOwner::Any)
        .map_err(|source| SandboxModeWriteError::Write {
            path: path.clone(),
            source,
        })?;
    Ok(path)
}

/// How long a mode write waits for another writer of the same folder's file before it fails.
#[cfg(all(unix, not(test)))]
const MODE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(all(unix, test))]
const MODE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

#[cfg(test)]
type CheckHook = Option<Box<dyn FnMut()>>;

#[cfg(test)]
thread_local! {
    /// Runs between the user-layer path check and the open in [`write_workspace_sandbox_mode_in`].
    static AFTER_USER_LAYER_CHECK: std::cell::RefCell<CheckHook> =
        const { std::cell::RefCell::new(None) };
}

/// Largest `workspaced.toml` read. The workspace copy is repository content, so neither the
/// resolver nor `sandbox.mode.set` reads it without a bound.
const MAX_WORKSPACED_TOML_BYTES: u64 = 1024 * 1024;

/// One `workspaced.toml` at a link-free `path` ([`read_layer_text`] resolves), read through a
/// [`HeldDir`] from the root down without blocking, up to [`MAX_WORKSPACED_TOML_BYTES`]
/// (`FileTooLarge`), and only a regular file owned as `owner` asks by `fstat` (`InvalidInput`).
fn read_workspaced_toml(path: &Path, owner: FileOwner) -> std::io::Result<String> {
    let dir = path.parent().unwrap_or(path);
    let root = dir.ancestors().last().unwrap_or(dir);
    HeldDir::open(root, dir, FileOwner::Any, false)?.read(
        path.file_name().unwrap_or_default(),
        MAX_WORKSPACED_TOML_BYTES,
        owner,
    )
}

/// One layer's `[sandbox] mode` (file owned as `owner` asks): `Ok(None)` when absent or naming no
/// mode, `Err` with one warning when refused or unreadable ([`RefusedLayers`]). Read verbatim: no
/// `$VAR` expansion, so a file cannot pull the daemon's environment into a mode or a log line.
fn read_mode_layer(
    path: &Path,
    anchor: &Path,
    owner: FileOwner,
    inputs: &SandboxModeInputs<'_>,
) -> Result<Option<SandboxMode>, std::io::Error> {
    read_layer_mode(path, anchor, owner, inputs).inspect_err(|refusal| {
        let outcome = if inputs.backend_available {
            "this layer counts as sandbox mode enforce"
        } else {
            "no sandbox backend here, so the other layers decide (off when none names a mode)"
        };
        tracing::warn!(
            path = %path.display(),
            %refusal,
            "sandbox mode file is refused, not read: {outcome} until it can be read"
        );
    })
}

fn read_layer_mode(
    path: &Path,
    anchor: &Path,
    owner: FileOwner,
    inputs: &SandboxModeInputs<'_>,
) -> Result<Option<SandboxMode>, std::io::Error> {
    let text = match read_layer_text(path, anchor, owner, inputs) {
        Ok(text) => text,
        Err(error) if is_absent(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let root = toml::from_str::<toml::Value>(&text).map_err(|error| {
        // `toml_error_detail` is built from the span; `Display` would echo the source line
        let detail = xai_grok_config::toml_error_detail(&text, &error);
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("not valid TOML: {detail}"),
        )
    })?;
    Ok(SandboxSettings::from_config_document(&root).and_then(|settings| settings.mode))
}

/// The layer file's text, `path` lying below `anchor` (the served folder, or the grok home's
/// parent so a planted `.grok` link is judged); through a link only its real path is read, and
/// only where no command may write ([`real_path_below`]).
fn read_layer_text(
    path: &Path,
    anchor: &Path,
    owner: FileOwner,
    inputs: &SandboxModeInputs<'_>,
) -> std::io::Result<String> {
    let holds = |real: &Path| {
        inputs
            .writable
            .holds(real, inputs.workspace_root, inputs.grok_home)
    };
    let file = match real_path_below(path, anchor, holds)? {
        Some(real) => real,
        None => canonical_path(anchor).join(path.strip_prefix(anchor).unwrap_or(path)),
    };
    read_workspaced_toml(&file, owner)
}

/// Where `path`, below `anchor`, really is when a component between them (`path` included) is a
/// link; `None` when none is. A dangling link, or a real path `holds` (writable by a command),
/// is `InvalidInput`; a path cut short past that check (missing, not a directory) is that error.
fn real_path_below(
    path: &Path,
    anchor: &Path,
    holds: impl Fn(&Path) -> std::io::Result<bool>,
) -> std::io::Result<Option<PathBuf>> {
    let refused = |reason: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, reason);
    let mut below: Vec<&Path> = path
        .ancestors()
        .take_while(|at| *at != anchor && at.starts_with(anchor))
        .collect();
    below.reverse();
    let (mut linked, mut cut) = (false, None);
    for at in below {
        match std::fs::symlink_metadata(at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                std::fs::metadata(at).map_err(|error| {
                    refused(format!(
                        "the symlink {} does not resolve: {error}",
                        at.display()
                    ))
                })?;
                linked = true;
            }
            Ok(_) => {}
            Err(error) if is_absent(&error) => {
                cut = Some(error);
                break;
            }
            Err(error) => return Err(error),
        }
    }
    if !linked {
        return cut.map_or(Ok(None), Err);
    }
    let real = canonical_path(path);
    let writable = holds(&real).map_err(|error| {
        refused(format!(
            "where a sandboxed command may write cannot be told: {error}"
        ))
    })?;
    if writable {
        return Err(refused(format!(
            "a symlink on the way resolves to {}, where a sandboxed command may write",
            real.display()
        )));
    }
    cut.map_or(Ok(Some(real)), Err)
}

/// A layer file that is not there: missing, or a component on the way that is not a directory.
fn is_absent(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

#[cfg(test)]
#[path = "sandbox_mode_tests.rs"]
mod tests;
