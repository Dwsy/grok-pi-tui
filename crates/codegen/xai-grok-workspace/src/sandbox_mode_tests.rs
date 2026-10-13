#[cfg(unix)]
use std::os::unix::fs::FileTypeExt as _;
use std::path::Path;

use xai_grok_config_types::RemoteSettings;
use xai_grok_sandbox::command::mode::{ModeDegradation, ResolvedSandboxMode, SandboxModeSource};
use xai_grok_sandbox::command::{SandboxMode, WritableLocations};

use super::{
    SandboxModeInputs, SandboxModeWriteError, resolve_sandbox_mode_in, set_workspace_mode_in,
    write_workspace_sandbox_mode,
};
use crate::folder_trust::{self, TrustOutcome};
use crate::trust::workspace_key;

fn write_sandbox_table(dir: &Path, rel: &str, mode: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("[sandbox]\nmode = \"{mode}\"\n")).unwrap();
}

/// The layers on a host with a backend; the degradation rule is exercised where it is tested.
fn resolve(ws: &Path, home: &Path, env: Option<&str>) -> ResolvedSandboxMode {
    resolve_with(ws, home, env, None, true)
}

/// No temporary directory and no build-cache tree counts as writable here: the fixtures live
/// in the host's temp dir, which the daemon's own set holds.
fn resolve_with(
    ws: &Path,
    home: &Path,
    env: Option<&str>,
    remote: Option<&RemoteSettings>,
    backend_available: bool,
) -> ResolvedSandboxMode {
    resolve_sandbox_mode_in(SandboxModeInputs {
        workspace_root: ws,
        grok_home: home,
        env,
        remote,
        backend_available,
        writable: &WritableLocations::new(None, &[]),
    })
}

/// The layers on a host with a backend, with `writable` as the places a command may write.
fn resolve_in(ws: &Path, home: &Path, writable: &WritableLocations) -> ResolvedSandboxMode {
    resolve_sandbox_mode_in(SandboxModeInputs {
        workspace_root: ws,
        grok_home: home,
        env: None,
        remote: None,
        backend_available: true,
        writable,
    })
}

/// The mode a refused or unreadable layer that decided gives.
fn strictest(mode: SandboxMode, source: SandboxModeSource) -> ResolvedSandboxMode {
    ResolvedSandboxMode {
        mode,
        source,
        degraded: Some(ModeDegradation::ConfigRefused),
    }
}

#[test]
fn no_files_no_env_resolves_to_the_default() {
    let tmp = tempfile::tempdir().unwrap();
    let resolved = resolve(&tmp.path().join("ws"), &tmp.path().join("home"), None);
    assert_eq!(SandboxMode::Off, resolved.mode);
    assert_eq!(SandboxModeSource::Default, resolved.source);
}

#[test]
fn user_workspaced_toml_sets_the_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    write_sandbox_table(&home, "workspaced.toml", "enforce");
    let resolved = resolve(&tmp.path().join("ws"), &home, None);
    assert_eq!(SandboxMode::Enforce, resolved.mode);
    assert_eq!(SandboxModeSource::UserConfig, resolved.source);
}

/// A cloned repository's `[sandbox] mode = "off"` cannot lower the
/// user's `enforce`.
#[test]
fn workspace_grok_workspaced_toml_cannot_lower_the_user_file() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    write_sandbox_table(&home, "workspaced.toml", "enforce");
    write_sandbox_table(&ws, ".grok/workspaced.toml", "off");
    let resolved = resolve(&ws, &home, None);
    assert_eq!(SandboxMode::Enforce, resolved.mode);
    assert_eq!(SandboxModeSource::UserConfig, resolved.source);
}

#[test]
fn workspace_grok_workspaced_toml_tightens_the_user_file() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    write_sandbox_table(&home, "workspaced.toml", "observe");
    write_sandbox_table(&ws, ".grok/workspaced.toml", "enforce");
    let resolved = resolve(&ws, &home, None);
    assert_eq!(SandboxMode::Enforce, resolved.mode);
    assert_eq!(SandboxModeSource::WorkspaceConfig, resolved.source);
}

/// The workspace layer is read whether or not the folder is trusted: it can only tighten. A
/// checkout whose own `.envrc` makes it untrusted (a repo-local config with no recorded trust
/// grant) still gets the `enforce` its `.grok/workspaced.toml` asks for; gated on trust, that
/// file would drop the mode to the user's default.
#[test]
fn an_untrusted_folder_still_tightens_through_its_workspace_file() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    write_sandbox_table(&ws, ".grok/workspaced.toml", "enforce");
    std::fs::write(ws.join(".envrc"), "export FOO=1\n").unwrap();
    let trust = folder_trust::decide_inputs_with_interactive(&ws, &workspace_key(&ws), false);
    assert_eq!(
        TrustOutcome::Untrusted,
        folder_trust::decide(true, &trust),
        "the fixture folder must be one the trust decision refuses"
    );
    let resolved = resolve(&ws, &home, None);
    assert_eq!(SandboxMode::Enforce, resolved.mode);
    assert_eq!(SandboxModeSource::WorkspaceConfig, resolved.source);
}

#[test]
fn remote_rollout_switch_beats_the_user_file_and_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    write_sandbox_table(&home, "workspaced.toml", "enforce");
    write_sandbox_table(&ws, ".grok/workspaced.toml", "enforce");
    let remote = RemoteSettings {
        sandbox_mode: Some(SandboxMode::Observe),
        ..RemoteSettings::default()
    };
    let resolved = resolve_with(&ws, &home, None, Some(&remote), true);
    assert_eq!(SandboxMode::Observe, resolved.mode);
    assert_eq!(SandboxModeSource::Remote, resolved.source);
    assert_eq!(None, resolved.degraded);
}

/// With no backend the rollout switch's `enforce` runs `off`, marked and warned about once; the
/// developer's own `enforce` (their user file here), with or without the switch, keeps `enforce`
/// and refuses. With a backend the switch's `enforce` stands.
#[test]
fn the_rollout_switch_enforce_runs_as_off_without_a_backend_and_the_users_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    let remote = RemoteSettings {
        sandbox_mode: Some(SandboxMode::Enforce),
        ..RemoteSettings::default()
    };
    let (degraded, log) =
        crate::capturing_warn_logs(|| resolve_with(&ws, &home, None, Some(&remote), false));
    assert_eq!(
        ResolvedSandboxMode {
            mode: SandboxMode::Off,
            source: SandboxModeSource::Remote,
            degraded: Some(ModeDegradation::EnforceWithoutBackend),
        },
        degraded
    );
    assert_eq!(1, log.matches("WARN").count(), "{log}");
    assert!(log.contains("no sandbox backend"), "{log}");

    let with_backend = resolve_with(&ws, &home, None, Some(&remote), true);
    assert_eq!(
        ResolvedSandboxMode::new(SandboxMode::Enforce, SandboxModeSource::Remote),
        with_backend
    );

    write_sandbox_table(&home, "workspaced.toml", "enforce");
    for remote in [None, Some(&remote)] {
        let (users, log) =
            crate::capturing_warn_logs(|| resolve_with(&ws, &home, None, remote, false));
        assert_eq!(
            ResolvedSandboxMode::new(SandboxMode::Enforce, SandboxModeSource::UserConfig),
            users,
            "{remote:?}"
        );
        assert_eq!(
            "", log,
            "nothing to warn about: refusing is what they asked for"
        );
    }
}

#[test]
fn env_value_overrides_every_file() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    write_sandbox_table(&home, "workspaced.toml", "off");
    write_sandbox_table(&ws, ".grok/workspaced.toml", "off");
    let resolved = resolve(&ws, &home, Some("enforce"));
    assert_eq!(SandboxMode::Enforce, resolved.mode);
    assert_eq!(SandboxModeSource::Env, resolved.source);
}

/// What a malformed workspace file would set is unknown, so it is the strictest mode rather than
/// falling through to the user's `observe`.
#[test]
fn a_malformed_workspace_file_is_the_strictest_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    write_sandbox_table(&home, "workspaced.toml", "observe");
    let broken = ws.join(".grok").join("workspaced.toml");
    std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
    std::fs::write(&broken, "[sandbox\nmode = ").unwrap();
    assert_eq!(
        strictest(SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig),
        resolve(&ws, &home, None)
    );
}

#[test]
fn misspelt_mode_in_the_user_file_is_unset_not_off() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    write_sandbox_table(&home, "workspaced.toml", "enforcee");
    let resolved = resolve(&tmp.path().join("ws"), &home, None);
    assert_eq!(SandboxMode::Off, resolved.mode);
    assert_eq!(SandboxModeSource::Default, resolved.source);
}

#[test]
fn other_tables_and_unknown_keys_beside_mode_do_not_disturb_it() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("workspaced.toml"),
        "[sandbox]\nfuture_key = 1\nmode = \"off\"\n\n[proxy]\nport = 3128\n",
    )
    .unwrap();
    let resolved = resolve(&tmp.path().join("ws"), &home, None);
    assert_eq!(SandboxMode::Off, resolved.mode);
    assert_eq!(SandboxModeSource::UserConfig, resolved.source);
}

/// The mode lives in the daemon's own file. A `[sandbox] mode` in the CLI's
/// `config.toml` — user or workspace — is not a layer and never decides anything.
#[test]
fn the_cli_config_toml_is_never_read() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    write_sandbox_table(&home, "config.toml", "enforce");
    write_sandbox_table(&ws, ".grok/config.toml", "enforce");
    let resolved = resolve(&ws, &home, None);
    assert_eq!(SandboxMode::Off, resolved.mode);
    assert_eq!(SandboxModeSource::Default, resolved.source);
}

#[test]
fn the_layer_files_are_workspaced_toml_under_the_home_and_the_folder() {
    assert_eq!(
        Path::new("/opt/ws-fixture/me/.grok/workspaced.toml"),
        super::user_config_path(Path::new("/opt/ws-fixture/me/.grok"))
    );
    assert_eq!(
        Path::new("/opt/ws-fixture/me/proj/.grok/workspaced.toml"),
        super::workspace_config_path(Path::new("/opt/ws-fixture/me/proj"))
    );
}

#[test]
fn write_creates_the_workspace_file_and_the_layer_reads_it_back() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let path = write_workspace_sandbox_mode(&ws, SandboxMode::Enforce).unwrap();
    assert_eq!(ws.join(".grok").join("workspaced.toml"), path);
    let resolved = resolve(&ws, &tmp.path().join("home"), None);
    assert_eq!(SandboxMode::Enforce, resolved.mode);
    assert_eq!(SandboxModeSource::WorkspaceConfig, resolved.source);
}

/// `sandbox.mode.set` as one step: the write, then the layers read again, so the answer is the
/// mode the folder is in now — `off` asked under a user layer of `observe` answers `observe` from
/// the user file, since the workspace layer only tightens. A root that is not a directory is
/// refused before anything is written: the write may create `.grok/`, never the folder.
#[test]
fn set_workspace_mode_writes_then_answers_with_the_mode_the_layers_resolve_to() {
    let tmp = tempfile::tempdir().unwrap();
    let (ws, home) = (tmp.path().join("ws"), tmp.path().join("home"));
    std::fs::create_dir_all(&ws).unwrap();
    write_sandbox_table(&home, "workspaced.toml", "observe");
    let inputs = SandboxModeInputs {
        workspace_root: &ws,
        grok_home: &home,
        env: None,
        remote: None,
        backend_available: true,
        writable: &WritableLocations::default(),
    };
    let (path, resolved) = set_workspace_mode_in(SandboxMode::Off, inputs).unwrap();
    assert_eq!(ws.join(".grok").join("workspaced.toml"), path);
    assert_eq!(SandboxMode::Observe, resolved.mode);
    assert_eq!(SandboxModeSource::UserConfig, resolved.source);
    let (_, resolved) = set_workspace_mode_in(SandboxMode::Enforce, inputs).unwrap();
    assert_eq!(SandboxMode::Enforce, resolved.mode);
    assert_eq!(SandboxModeSource::WorkspaceConfig, resolved.source);

    let missing = tmp.path().join("missing");
    let refused = set_workspace_mode_in(
        SandboxMode::Enforce,
        SandboxModeInputs {
            workspace_root: &missing,
            ..inputs
        },
    )
    .unwrap_err();
    assert!(
        matches!(&refused, SandboxModeWriteError::NotADirectory { path } if *path == missing),
        "{refused}"
    );
    assert!(!missing.exists(), "the refusal created the folder");
}

/// For the home folder the workspace file is the user layer every folder reads: the write is
/// refused, in any spelling of the home, so a folder's `sandbox.mode.set` never loosens every
/// folder's mode. The file is not created.
#[test]
fn write_refuses_the_home_folder_whose_workspace_file_is_the_user_layer() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let grok_home = home.join(".grok");
    std::fs::create_dir_all(&grok_home).unwrap();
    for spelling in [home.clone(), home.join("proj/..")] {
        let error = super::write_workspace_sandbox_mode_in(&spelling, &grok_home, SandboxMode::Off)
            .unwrap_err();
        assert!(
            matches!(error, super::SandboxModeWriteError::UserLayer { .. }),
            "{error}"
        );
    }
    assert!(!grok_home.join("workspaced.toml").exists());
    let ws = home.join("proj");
    std::fs::create_dir_all(&ws).unwrap();
    super::write_workspace_sandbox_mode_in(&ws, &grok_home, SandboxMode::Enforce).unwrap();
}

/// A folder root relinked to the home folder after the user-layer check is caught on the held
/// directory: the write is refused and the user layer is never written.
#[cfg(unix)]
#[test]
fn write_refuses_a_root_relinked_to_the_home_folder_after_the_check() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj, root) = (
        tmp.path().join("home"),
        tmp.path().join("proj"),
        tmp.path().join("root"),
    );
    let grok_home = home.join(".grok");
    for dir in [&grok_home, &proj] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::os::unix::fs::symlink(&proj, &root).unwrap();
    let (link, to) = (root.clone(), home.clone());
    super::AFTER_USER_LAYER_CHECK.set(Some(Box::new(move || {
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&to, &link).unwrap();
    })));
    let refused = super::write_workspace_sandbox_mode_in(&root, &grok_home, SandboxMode::Off);
    super::AFTER_USER_LAYER_CHECK.set(None);
    assert!(
        matches!(refused, Err(super::SandboxModeWriteError::UserLayer { .. })),
        "{refused:?}"
    );
    assert!(!grok_home.join("workspaced.toml").exists());
    std::fs::remove_file(&root).unwrap();
    std::os::unix::fs::symlink(&proj, &root).unwrap();
    super::write_workspace_sandbox_mode_in(&root, &grok_home, SandboxMode::Off).unwrap();
    assert!(proj.join(".grok/workspaced.toml").exists());
}

/// On APFS a grok home spelled `.Grok` that does not exist yet is the home folder's `.grok`.
#[cfg(target_os = "macos")]
#[test]
fn write_refuses_the_home_folder_when_the_missing_grok_home_differs_only_in_case() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let error =
        super::write_workspace_sandbox_mode_in(&home, &home.join(".Grok"), SandboxMode::Off)
            .unwrap_err();
    assert!(
        matches!(error, super::SandboxModeWriteError::UserLayer { .. }),
        "{error}"
    );
    assert!(!home.join(".grok").exists());
}

#[test]
fn write_keeps_other_keys_comments_and_replaces_an_existing_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let path = ws.join(".grok").join("workspaced.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "# workspace settings\n[proxy]\nport = 3128\n\n[sandbox]\nfuture_key = 1 # keep\nmode = \"enforce\"\n",
    )
    .unwrap();
    write_workspace_sandbox_mode(&ws, SandboxMode::Observe).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# workspace settings\n"), "{text}");
    assert!(text.contains("port = 3128"), "{text}");
    assert!(text.contains("future_key = 1 # keep"), "{text}");
    assert!(text.contains("mode = \"observe\""), "{text}");
    assert!(!text.contains("enforce"), "{text}");
    assert_eq!(
        SandboxMode::Observe,
        resolve(&ws, &tmp.path().join("home"), None).mode
    );
}

/// Writers of one folder's file take turns on its `.grok` directory: one behind a lock held there
/// fails after a bounded wait with the file untouched, and concurrent writers all succeed and
/// leave the file holding one of their modes, never removed as a swapped-in file.
#[cfg(unix)]
#[test]
fn concurrent_mode_writers_take_turns_and_the_file_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let (ws, grok_home) = (tmp.path().join("ws"), tmp.path().join("home/.grok"));
    std::fs::create_dir_all(&ws).unwrap();
    let write = |mode| super::write_workspace_sandbox_mode_in(&ws, &grok_home, mode);
    let path = write(SandboxMode::Observe).unwrap();
    let holder = std::fs::File::open(ws.join(".grok")).unwrap();
    fs2::FileExt::lock_exclusive(&holder).unwrap();
    let started = std::time::Instant::now();
    let held = write(SandboxMode::Enforce).unwrap_err();
    assert!(
        matches!(&held, super::SandboxModeWriteError::Write { source, .. } if source.kind() == std::io::ErrorKind::TimedOut),
        "{held}"
    );
    assert!(
        started.elapsed() < super::MODE_LOCK_WAIT * 4,
        "bounded wait"
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("mode = \"observe\"")
    );
    fs2::FileExt::unlock(&holder).unwrap();

    std::thread::scope(|scope| {
        let writers: Vec<_> = [SandboxMode::Enforce, SandboxMode::Observe]
            .into_iter()
            .map(|mode| scope.spawn(move || (0..25).try_for_each(|_| write(mode).map(drop))))
            .collect();
        for writer in writers {
            writer.join().unwrap().unwrap();
        }
    });
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("mode = \"enforce\"") || text.contains("mode = \"observe\""),
        "{text}"
    );
}

#[test]
fn write_refuses_a_file_that_is_not_toml_and_a_non_table_sandbox_key() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let path = ws.join(".grok").join("workspaced.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[sandbox\nmode = ").unwrap();
    let error = write_workspace_sandbox_mode(&ws, SandboxMode::Off).unwrap_err();
    assert!(error.to_string().contains("not valid TOML"), "{error}");
    std::fs::write(&path, "sandbox = 3\n").unwrap();
    let error = write_workspace_sandbox_mode(&ws, SandboxMode::Off).unwrap_err();
    assert!(error.to_string().contains("not a table"), "{error}");
    assert_eq!("sandbox = 3\n", std::fs::read_to_string(&path).unwrap());
}

/// A workspace file past the read cap is never read whole: the layer is unreadable, so it counts
/// as the strictest mode rather than falling through to the user's `observe`, and
/// `sandbox.mode.set` refuses to rewrite it.
#[test]
fn an_oversized_workspace_file_is_the_strictest_mode_and_never_rewritten() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    write_sandbox_table(&home, "workspaced.toml", "observe");
    let path = ws.join(".grok").join("workspaced.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let padding = "#".repeat(usize::try_from(super::MAX_WORKSPACED_TOML_BYTES).unwrap());
    let contents = format!("[sandbox]\nmode = \"enforce\"\n{padding}\n");
    std::fs::write(&path, &contents).unwrap();
    assert_eq!(
        strictest(SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig),
        resolve(&ws, &home, None)
    );
    let error = write_workspace_sandbox_mode(&ws, SandboxMode::Off).unwrap_err();
    assert!(
        matches!(&error, super::SandboxModeWriteError::Read { source, .. } if source.kind() == std::io::ErrorKind::FileTooLarge),
        "{error}"
    );
    assert_eq!(
        contents.len(),
        std::fs::read_to_string(&path).unwrap().len()
    );
}

/// `[sandbox]` is read verbatim: a `$VAR` reference — with a default, or naming a variable that
/// is set — is text that names no mode, so neither layer can make the daemon read its
/// environment into the mode. The user's `observe` beneath is what applies.
#[test]
fn env_references_in_a_layer_are_not_expanded() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    write_sandbox_table(&home, "workspaced.toml", "observe");
    for reference in [
        "${GROK_SANDBOX_MODE_TEST_UNSET_VAR:-enforce}",
        "$PATH",
        "${HOME}",
    ] {
        write_sandbox_table(&ws, ".grok/workspaced.toml", reference);
        let resolved = resolve(&ws, &home, None);
        assert_eq!(SandboxMode::Observe, resolved.mode, "{reference}");
        assert_eq!(
            SandboxModeSource::UserConfig,
            resolved.source,
            "{reference}"
        );
    }
}

/// A value that names no mode is logged as invalid; its text — repository content, or whatever
/// a `$VAR` would have stood for — never reaches the log.
#[test]
fn an_invalid_mode_is_logged_without_its_text() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    let secret = "hunter2-not-a-mode";
    write_sandbox_table(&ws, ".grok/workspaced.toml", secret);
    let (resolved, log) = crate::capturing_warn_logs(|| resolve(&ws, &home, None));
    assert_eq!(SandboxMode::Off, resolved.mode);
    assert_eq!(SandboxModeSource::Default, resolved.source);
    assert!(log.contains("invalid sandbox mode"), "one warning: {log}");
    assert!(!log.contains(secret), "the value is echoed: {log}");
    assert!(!log.contains("hunter2"), "the value is echoed: {log}");
}

/// A FIFO in the workspace file's place — a command's to plant — must not stall the resolver
/// (the open blocks until a writer appears) or `sandbox.mode.set`: the layer is refused, so it
/// counts as the strictest mode, and the write refuses to read it, with no writer ever attached.
#[cfg(unix)]
#[test]
fn a_fifo_in_the_workspace_files_place_does_not_stall_the_resolver_or_the_writer() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    write_sandbox_table(&home, "workspaced.toml", "observe");
    let path = ws.join(".grok").join("workspaced.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let fifo = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `fifo` is a valid NUL-terminated path and `mkfifo` reads nothing else.
    assert_eq!(0, unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) });

    let (sender, receiver) = std::sync::mpsc::channel();
    let (ws_for_thread, home_for_thread) = (ws.clone(), home.clone());
    std::thread::spawn(move || {
        let resolved = resolve(&ws_for_thread, &home_for_thread, None);
        let written = write_workspace_sandbox_mode(&ws_for_thread, SandboxMode::Enforce);
        let _ = sender.send((resolved, written.map_err(|error| error.to_string())));
    });
    let (resolved, written) = receiver
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the resolver or the writer blocked on the FIFO");
    assert_eq!(
        strictest(SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig),
        resolved
    );
    let error = written.unwrap_err();
    assert!(error.contains("not a regular file"), "{error}");
    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_fifo(),
        "the FIFO was replaced"
    );
}

/// A layer that is a symlink resolving where a command may write — the workspace, a build-cache
/// tree, a folder a grant opened, another served folder, the sessions tree — is refused, so it
/// is the strictest mode with one warning: a command there could rewrite the mode. When the
/// grants cannot be read, where a command may write is unknown and the link is refused too.
/// `sandbox.mode.set` never writes through a link, at the file or at `.grok`, wherever it points:
/// the link and its target stay as they were. A planted `.grok`, or a grok home, linking into
/// the workspace or a build-cache tree is refused the same way.
#[cfg(unix)]
#[test]
fn a_link_into_a_place_a_command_may_write_is_the_strictest_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    let user_home = tmp.path().join("user");
    let granted = tmp.path().join("granted");
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&ws).unwrap();
    xai_grok_config::ensure_sessions_cwd_dir_in(&home, other.to_str().unwrap()).unwrap();
    let grants = home.join("sandbox_grants.toml");
    let granted_root = toml::Value::from(granted.to_str().unwrap());
    std::fs::write(
        &grants,
        format!("[[grant]]\nsubject = {{ kind = \"fs_write_root\", root = {granted_root} }}\n"),
    )
    .unwrap();
    let writable = WritableLocations::new(Some(&user_home), &[]);
    let layer = home.join("workspaced.toml");
    for place in [
        ws.join("planted"),
        user_home.join(".cargo/registry/planted"),
        granted.join("planted"),
        other.join("planted"),
        home.join("sessions"),
    ] {
        write_sandbox_table(&place, "observe.toml", "observe");
        std::os::unix::fs::symlink(place.join("observe.toml"), &layer).unwrap();
        let (resolved, log) = crate::capturing_warn_logs(|| resolve_in(&ws, &home, &writable));
        let place = place.display();
        assert_eq!(
            strictest(SandboxMode::Enforce, SandboxModeSource::UserConfig),
            resolved,
            "{place}"
        );
        assert_eq!(1, log.matches("is refused, not read").count(), "{log}");
        assert!(log.contains("where a sandboxed command may write"), "{log}");
        std::fs::remove_file(&layer).unwrap();
    }

    write_sandbox_table(tmp.path(), "dotfiles/observe.toml", "observe");
    std::os::unix::fs::symlink(tmp.path().join("dotfiles/observe.toml"), &layer).unwrap();
    std::fs::write(&grants, "[[grant]\n").unwrap();
    let (resolved, log) = crate::capturing_warn_logs(|| resolve_in(&ws, &home, &writable));
    assert_eq!(
        strictest(SandboxMode::Enforce, SandboxModeSource::UserConfig),
        resolved
    );
    assert!(log.contains("cannot be told"), "{log}");
    std::fs::remove_file(&grants).unwrap();
    assert_eq!(
        ResolvedSandboxMode::new(SandboxMode::Observe, SandboxModeSource::UserConfig),
        resolve_in(&ws, &home, &writable)
    );

    let target = ws.join("planted/off.toml");
    write_sandbox_table(&ws, "planted/off.toml", "off");
    let ws_layer = ws.join(".grok").join("workspaced.toml");
    std::fs::create_dir_all(ws_layer.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &ws_layer).unwrap();
    assert_eq!(
        strictest(SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig),
        resolve_in(&ws, &home, &writable)
    );

    let before = std::fs::read(&target).unwrap();
    let error = super::write_workspace_sandbox_mode_in(&ws, &home, SandboxMode::Off).unwrap_err();
    assert!(error.to_string().contains("symlink"), "{error}");
    assert_eq!(before, std::fs::read(&target).unwrap());
    assert!(
        std::fs::symlink_metadata(&ws_layer)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    std::fs::remove_dir_all(ws.join(".grok")).unwrap();
    let dotfile = tmp.path().join("dotfiles/grok/workspaced.toml");
    write_sandbox_table(tmp.path(), "dotfiles/grok/workspaced.toml", "enforce");
    std::os::unix::fs::symlink(dotfile.parent().unwrap(), ws.join(".grok")).unwrap();
    let before = std::fs::read(&dotfile).unwrap();
    let error = super::write_workspace_sandbox_mode_in(&ws, &home, SandboxMode::Off).unwrap_err();
    assert!(error.to_string().contains("is a symlink"), "{error}");
    assert_eq!(before, std::fs::read(&dotfile).unwrap());
    std::fs::remove_file(ws.join(".grok")).unwrap();

    let linked_home = tmp.path().join("linked-home");
    std::os::unix::fs::symlink(ws.join("planted-home"), &linked_home).unwrap();
    write_sandbox_table(&ws, "planted-home/workspaced.toml", "off");
    let (resolved, log) = crate::capturing_warn_logs(|| resolve_in(&ws, &linked_home, &writable));
    // The managed layer under the linked home is refused too, and it outranks the user layer
    let expected = strictest(SandboxMode::Enforce, SandboxModeSource::Remote);
    assert_eq!(expected, resolved);
    assert!(log.contains("where a sandboxed command may write"), "{log}");
    for place in [ws.join("planted"), user_home.join(".cargo/registry/grok")] {
        write_sandbox_table(&place, "workspaced.toml", "off");
        std::os::unix::fs::symlink(&place, ws.join(".grok")).unwrap();
        let (resolved, log) = crate::capturing_warn_logs(|| resolve_in(&ws, &home, &writable));
        let shown = place.display();
        let expected = strictest(SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig);
        assert_eq!(expected, resolved, "{shown}");
        assert!(log.contains("where a sandboxed command may write"), "{log}");
        std::fs::remove_file(ws.join(".grok")).unwrap();
    }
}

/// A layer refused or unreadable (a directory or dangling link in its place, not TOML or UTF-8) is
/// `enforce`, warned once, `degraded: config_refused` (no backend: the readable layers', marked),
/// never `off`; absent again once gone, or when `.grok` is a file or a link to one.
#[cfg(unix)]
#[test]
fn a_refused_or_unreadable_layer_is_the_strictest_mode_with_one_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    let layer = home.join("workspaced.toml");
    std::fs::create_dir_all(&layer).unwrap();
    let (resolved, log) = crate::capturing_warn_logs(|| resolve(&ws, &home, None));
    assert_eq!(
        strictest(SandboxMode::Enforce, SandboxModeSource::UserConfig),
        resolved
    );
    assert_eq!(1, log.matches("is refused, not read").count(), "{log}");
    assert!(log.contains(&*layer.to_string_lossy()), "{log}");
    assert!(log.contains("not a regular file"), "{log}");
    assert_eq!(
        serde_json::json!({"mode": "enforce", "source": "user_config", "degraded": "config_refused"}),
        serde_json::to_value(resolved).unwrap()
    );

    std::fs::remove_dir(&layer).unwrap();
    std::os::unix::fs::symlink(tmp.path().join("nowhere.toml"), &layer).unwrap();
    for (backend_available, mode, outcome) in [
        (true, SandboxMode::Enforce, "counts as sandbox mode enforce"),
        (false, SandboxMode::Off, "no sandbox backend here"),
    ] {
        let (resolved, log) =
            crate::capturing_warn_logs(|| resolve_with(&ws, &home, None, None, backend_available));
        assert_eq!(
            strictest(mode, SandboxModeSource::UserConfig),
            resolved,
            "backend_available = {backend_available}"
        );
        assert_eq!(1, log.matches("is refused, not read").count(), "{log}");
        assert!(
            log.contains("does not resolve") && log.contains(outcome),
            "{log}"
        );
    }

    std::fs::remove_file(&layer).unwrap();
    std::fs::write(&layer, "[sandbox\nmode = \"off\"\n").unwrap();
    let (resolved, log) = crate::capturing_warn_logs(|| resolve(&ws, &home, None));
    assert_eq!(
        strictest(SandboxMode::Enforce, SandboxModeSource::UserConfig),
        resolved
    );
    assert_eq!(1, log.matches("is refused, not read").count(), "{log}");
    assert!(log.contains("not valid TOML"), "{log}");

    write_sandbox_table(&home, "workspaced.toml", "observe");
    let ws_layer = ws.join(".grok").join("workspaced.toml");
    std::fs::create_dir_all(ws_layer.parent().unwrap()).unwrap();
    std::fs::write(&ws_layer, b"[sandbox]\nmode = \"off\" # \xff\n").unwrap();
    for (backend_available, mode) in [(true, SandboxMode::Enforce), (false, SandboxMode::Observe)] {
        let (resolved, log) =
            crate::capturing_warn_logs(|| resolve_with(&ws, &home, None, None, backend_available));
        assert_eq!(
            strictest(mode, SandboxModeSource::WorkspaceConfig),
            resolved,
            "backend_available = {backend_available}"
        );
        assert_eq!(1, log.matches("is refused, not read").count(), "{log}");
    }

    std::fs::remove_dir_all(ws.join(".grok")).unwrap();
    let file = tmp.path().join("not-settings");
    std::fs::write(&file, "a file, not the folder's settings").unwrap();
    for linked in [false, true] {
        if linked {
            std::os::unix::fs::symlink(&file, ws.join(".grok")).unwrap();
        } else {
            std::fs::copy(&file, ws.join(".grok")).unwrap();
        }
        for backend_available in [true, false] {
            let (resolved, log) = crate::capturing_warn_logs(|| {
                resolve_with(&ws, &home, None, None, backend_available)
            });
            assert_eq!(
                ResolvedSandboxMode::new(SandboxMode::Observe, SandboxModeSource::UserConfig),
                resolved,
                "linked = {linked}, backend_available = {backend_available}"
            );
            assert_eq!("", log);
        }
        std::fs::remove_file(ws.join(".grok")).unwrap();
    }
    std::fs::remove_file(&layer).unwrap();
    assert_eq!(
        ResolvedSandboxMode::new(SandboxMode::Off, SandboxModeSource::Default),
        resolve(&ws, &home, None)
    );
}

/// A refused or unreadable layer never ties or lowers a readable `enforce`: beside it the
/// readable layer decided, unmarked, and on a host with no backend it keeps refusing, as it
/// does beside a readable file. Only with both files refused or unreadable is the mode the
/// strictest, marked (`off` on a host with no backend, where no readable layer chose one).
#[cfg(unix)]
#[test]
fn a_refused_layer_never_lowers_or_ties_a_readable_enforce() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    let user_layer = home.join("workspaced.toml");
    let ws_layer = ws.join(".grok").join("workspaced.toml");
    let place = |path: &Path, state: &str| {
        if let Ok(metadata) = std::fs::symlink_metadata(path) {
            if metadata.is_dir() {
                std::fs::remove_dir(path).unwrap();
            } else {
                std::fs::remove_file(path).unwrap();
            }
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        match state {
            "refused" => std::fs::create_dir(path).unwrap(),
            "unreadable" => std::fs::write(path, "[sandbox\nmode = \"off\"\n").unwrap(),
            mode => std::fs::write(path, format!("[sandbox]\nmode = \"{mode}\"\n")).unwrap(),
        }
    };
    let enforce = |source| ResolvedSandboxMode::new(SandboxMode::Enforce, source);
    for backend_available in [true, false] {
        let now = || resolve_with(&ws, &home, None, None, backend_available);
        place(&ws_layer, "enforce");
        for (user, source) in [
            ("enforce", SandboxModeSource::UserConfig),
            ("refused", SandboxModeSource::WorkspaceConfig),
            ("unreadable", SandboxModeSource::WorkspaceConfig),
        ] {
            place(&user_layer, user);
            assert_eq!(
                enforce(source),
                now(),
                "user file {user}, backend_available = {backend_available}"
            );
        }
        place(&user_layer, "enforce");
        for workspace in ["refused", "unreadable"] {
            place(&ws_layer, workspace);
            assert_eq!(
                enforce(SandboxModeSource::UserConfig),
                now(),
                "workspace file {workspace}, backend_available = {backend_available}"
            );
        }
        let mode = if backend_available {
            SandboxMode::Enforce
        } else {
            SandboxMode::Off
        };
        for (user, workspace) in [
            ("refused", "refused"),
            ("unreadable", "refused"),
            ("refused", "unreadable"),
        ] {
            place(&user_layer, user);
            place(&ws_layer, workspace);
            assert_eq!(
                strictest(mode, SandboxModeSource::UserConfig),
                now(),
                "user file {user}, workspace file {workspace}, backend_available = \
                 {backend_available}"
            );
        }
    }
}

/// A layer that is a symlink to a file outside every place a command may write — a user's
/// dotfiles, through a linked directory too — is followed and its mode applies, unmarked and
/// with no warning. The target is read as the file itself would be: a FIFO there is refused
/// without blocking, so it is the strictest mode.
#[cfg(unix)]
#[test]
fn a_dotfile_link_outside_every_writable_place_is_read() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    let user_home = tmp.path().join("user");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    write_sandbox_table(&user_home, "dotfiles/grok/observe.toml", "observe");
    std::os::unix::fs::symlink(user_home.join("dotfiles"), user_home.join(".dotfiles")).unwrap();
    let writable = WritableLocations::new(Some(&user_home), &[]);
    let layer = home.join("workspaced.toml");
    std::os::unix::fs::symlink(user_home.join(".dotfiles/grok/observe.toml"), &layer).unwrap();
    let (resolved, log) = crate::capturing_warn_logs(|| resolve_in(&ws, &home, &writable));
    assert_eq!(
        ResolvedSandboxMode::new(SandboxMode::Observe, SandboxModeSource::UserConfig),
        resolved
    );
    assert!(!log.contains("refused"), "{log}");

    write_sandbox_table(&user_home, "dotfiles/grok/enforce.toml", "enforce");
    let ws_layer = ws.join(".grok").join("workspaced.toml");
    std::fs::create_dir_all(ws_layer.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(user_home.join("dotfiles/grok/enforce.toml"), &ws_layer).unwrap();
    assert_eq!(
        ResolvedSandboxMode::new(SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig),
        resolve_in(&ws, &home, &writable)
    );
    std::fs::remove_file(&ws_layer).unwrap();

    let enforce =
        ResolvedSandboxMode::new(SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig);
    write_sandbox_table(&ws, ".grok/workspaced.toml", "enforce");
    std::os::unix::fs::symlink(&ws, tmp.path().join("ws-link")).unwrap();
    assert_eq!(
        enforce,
        resolve_in(&tmp.path().join("ws-link"), &home, &writable)
    );
    std::fs::remove_dir_all(ws.join(".grok")).unwrap();
    write_sandbox_table(&user_home, "dotfiles/grok/workspaced.toml", "enforce");
    std::os::unix::fs::symlink(user_home.join(".dotfiles/grok"), ws.join(".grok")).unwrap();
    assert_eq!(enforce, resolve_in(&ws, &home, &writable));
    std::fs::remove_file(user_home.join("dotfiles/grok/workspaced.toml")).unwrap();
    assert_eq!(
        ResolvedSandboxMode::new(SandboxMode::Observe, SandboxModeSource::UserConfig),
        resolve_in(&ws, &home, &writable)
    );
    std::fs::remove_file(ws.join(".grok")).unwrap();

    let fifo_path = user_home.join("dotfiles/grok/fifo.toml");
    let fifo = std::ffi::CString::new(fifo_path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `fifo` is a valid NUL-terminated path and `mkfifo` reads nothing else.
    assert_eq!(0, unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) });
    std::fs::remove_file(&layer).unwrap();
    std::os::unix::fs::symlink(&fifo_path, &layer).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(resolve_in(&ws, &home, &writable));
    });
    let resolved = receiver
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the resolver blocked on the FIFO a link names");
    assert_eq!(
        strictest(SandboxMode::Enforce, SandboxModeSource::UserConfig),
        resolved
    );
}

/// `sandbox.mode.set` writes the daemon's file; a CLI `config.toml` beside
/// it is left byte-for-byte as it was.
#[test]
fn write_leaves_the_cli_config_toml_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let cli = ws.join(".grok").join("config.toml");
    std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
    let cli_text = "[sandbox]\nprofile = \"workspace\"\n";
    std::fs::write(&cli, cli_text).unwrap();
    let path = write_workspace_sandbox_mode(&ws, SandboxMode::Enforce).unwrap();
    assert_eq!(ws.join(".grok").join("workspaced.toml"), path);
    assert_eq!(cli_text, std::fs::read_to_string(&cli).unwrap());
    assert_eq!(
        "[sandbox]\nmode = \"enforce\"\n",
        std::fs::read_to_string(&path).unwrap()
    );
}
