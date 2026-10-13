use super::*;
use crate::command::grants::{Expiry, FixedClock, HostPattern, Provenance};
use crate::command::policy::{PolicyInputs, SandboxPolicy};
use crate::command::violation::Blocked;

const FIXTURE: &str = include_str!("../../tests/fixtures/sandbox_grants.toml");
const T0: i64 = 1_758_400_000;
/// The lock wait of the tests about the wait itself (a lock held still, a writer re-taking it);
/// every other test keeps [`GRANT_LOCK_WAIT`], so a slow disk under a loaded run fails none. The
/// re-taking writer holds the lock a tenth of it: one hold plus a loaded box's stalls must fit.
const SHORT_LOCK_WAIT: Duration = Duration::from_secs(2);

struct Home {
    grok_home: PathBuf,
    ws: PathBuf,
    /// The user's home the store is opened with, never the host's.
    user_home: PathBuf,
}

impl Home {
    fn new(tag: &str) -> Home {
        let root =
            std::env::temp_dir().join(format!("xai-sandbox-grants-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let grok_home = root.join(".grok");
        let ws = root.join("proj");
        let user_home = root.join("home/user");
        for dir in [&grok_home, &ws, &user_home] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Home {
            grok_home,
            ws,
            user_home,
        }
    }

    fn protected(&self) -> Vec<Protected> {
        protected::floor(&protected::ProtectedInputs {
            workspace_root: &ServedRoot::pin(&self.ws),
            grok_home: &self.grok_home,
            user_home: None,
            control_socket_dir: &self.grok_home.join("workspaced"),
            git_env: &GitConfigEnv::default(),
        })
    }

    async fn open(&self, clock: Arc<FixedClock>) -> GrantStore {
        GrantStore::open_in(
            &self.grok_home,
            &self.ws,
            self.protected(),
            Some(&self.user_home),
            clock,
        )
        .await
    }

    fn workspace_file(&self) -> PathBuf {
        xai_grok_config::sessions_cwd_dir_in(&self.grok_home, &self.ws.to_string_lossy())
            .join(GLOBAL_GRANTS_FILENAME)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        if let Some(root) = self.grok_home.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn grant(id: &str, subject: GrantSubject, scope: GrantScope, expires: Expiry) -> Grant {
    Grant {
        id: GrantId::new(id),
        subject,
        scope,
        expires,
        decision: GrantDecision::Allow,
        granted_at: T0,
        granted_by: "cli".to_owned(),
        via: Some(Provenance {
            command: "npm ci".to_owned(),
            tool_call_id: "tc-1".to_owned(),
        }),
    }
}

fn write_root(root: &str) -> GrantSubject {
    GrantSubject::FsWriteRoot {
        root: PathBuf::from(root),
    }
}

/// A lock held elsewhere (a sandboxed command can `flock` the readable sidecar) fails the edit
/// after a bounded wait instead of stalling the store; memory stays equal to disk, and once the
/// lock is released the next edit goes through.
#[tokio::test]
async fn an_edit_behind_a_lock_held_elsewhere_fails_instead_of_waiting_forever() {
    let home = Home::new("held-lock");
    let mut store = home
        .open(Arc::new(FixedClock::at(T0)))
        .await
        .with_lock_wait(SHORT_LOCK_WAIT);
    let global = store.global_file().to_path_buf();
    assert_eq!(
        Some(std::ffi::OsStr::new(protected::GRANTS_LOCK_FILENAME)),
        lock_path(&global).file_name(),
        "the sidecar the store locks is the one the floor protects"
    );
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path(&global))
        .unwrap();
    fs2::FileExt::lock_exclusive(&holder).unwrap();
    let row = |id: &str| {
        grant(
            id,
            write_root("/opt/fixture/elsewhere/held"),
            GrantScope::Global,
            Expiry::Never,
        )
    };
    let started = std::time::Instant::now();
    let error = store
        .add(row("0192c1a0-0000-7000-8000-00000000a001"))
        .await
        .expect_err("a held lock fails the edit");
    assert!(
        matches!(&error, GrantError::Io { source, .. } if source.kind() == std::io::ErrorKind::TimedOut),
        "{error}"
    );
    assert!(started.elapsed() < SHORT_LOCK_WAIT * 4, "bounded wait");
    assert!(store.live().is_empty(), "memory stays equal to disk");

    fs2::FileExt::unlock(&holder).unwrap();
    store
        .add(row("0192c1a0-0000-7000-8000-00000000a002"))
        .await
        .expect("released: the edit goes through");
    assert_eq!(1, store.live().len());
}

/// A writer in this process re-taking the lock the moment each edit released it (a second store's
/// burst) cannot starve this store's edits: each one's turn comes at the next release. The burst is
/// the store's own write path, holding the lock a tenth of the wait per edit, until three are in.
#[tokio::test]
async fn edits_are_not_starved_by_a_writer_re_taking_the_lock_back_to_back() {
    let home = Home::new("back-to-back");
    let store = home.open(Arc::new(FixedClock::at(T0))).await;
    let (persisted, rules) = (store.global.clone(), store.rules.clone());
    let mut store = store.with_lock_wait(SHORT_LOCK_WAIT);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let burst = std::thread::spawn({
        let stop = stop.clone();
        move || {
            let mut landed = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let row = grant(
                    &format!("0192c1a0-0000-7000-8000-0000000b{landed:04}"),
                    write_root(&format!("/opt/fixture/elsewhere/burst/r{landed}")),
                    GrantScope::Global,
                    Expiry::Never,
                );
                edit_rows_locked(&persisted, &rules, T0, Store::Create, |rows| {
                    std::thread::sleep(SHORT_LOCK_WAIT / 10);
                    rows.push(row);
                    true
                })
                .expect("the burst's own edits go through");
                landed += 1;
            }
            landed
        }
    });
    while std::fs::metadata(store.global_file()).is_err() {
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut added = Vec::new();
    for round in 0..3 {
        let row = grant(
            &format!("0192c1a0-0000-7000-8000-00000000b1{round:02}"),
            write_root(&format!("/opt/fixture/elsewhere/burst/mine{round}")),
            GrantScope::Global,
            Expiry::Never,
        );
        added.push(store.add(row).await);
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let landed = burst.join().unwrap();
    for added in added {
        added.expect("the edit's turn comes at the burst's next release");
    }
    let fresh = home.open(Arc::new(FixedClock::at(T0))).await;
    assert_eq!(
        landed + 3,
        fresh.live().len(),
        "every row of the burst, and the three edits'"
    );
}

/// The sidecar lock of the rows file at `path`, held until the handle drops.
fn hold_lock(path: &Path) -> std::fs::File {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path(path))
        .unwrap();
    fs2::FileExt::lock_exclusive(&holder).unwrap();
    holder
}

/// Polls `edit` once, far enough to hand its write to the blocking pool, then drops it.
async fn start_then_drop(edit: impl Future) {
    tokio::pin!(edit);
    tokio::select! {
        biased;
        _ = &mut edit => panic!("the edit waits on the held lock"),
        () = std::future::ready(()) => {}
    }
}

/// Waits, bounded, until the file at `path` reads as `settled` says, then until the write that
/// made it so has committed: an edit renames and stores its rows under the file's lock, and
/// releases it only after both.
async fn until_file(path: &Path, settled: impl Fn(&str) -> bool) {
    for _ in 0..500 {
        if std::fs::read_to_string(path).is_ok_and(|text| settled(&text)) {
            drop(hold_lock(path));
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{} never settled", path.display());
}

/// The policy a command in `home`'s workspace gets from the store's rows.
fn policy_of(home: &Home, store: &GrantStore) -> SandboxPolicy {
    let profile = crate::SandboxProfile {
        name: "workspace".to_owned(),
        read_only: vec![],
        read_write: vec![home.ws.clone()],
        deny: vec![],
        write_deny: vec![],
        default_read: true,
        restrict_network: false,
    };
    SandboxPolicy::build(PolicyInputs {
        workspace_root: &ServedRoot::pin(&home.ws),
        profile: &profile,
        grants: &allows_not_denied(&store.live()),
        proxy: None,
        tmp_dirs: &[],
        control_socket_dir: &home.grok_home.join("workspaced"),
        grok_home: &home.grok_home,
        user_home: None,
        git_env: &GitConfigEnv::default(),
    })
    .unwrap()
}

/// An edit whose caller is dropped while the blocking write waits on the file's lock (the
/// request went away, a `select!` took another branch) commits to memory with the write, never
/// before it: a dropped add is in neither `live` nor the policy until it lands, then in both; a
/// dropped revoke leaves the row live until it lands, then it is gone and stays gone.
#[tokio::test]
async fn an_edit_dropped_mid_write_commits_to_memory_with_the_write() {
    let home = Home::new("dropped-edit");
    let granted = home.ws.parent().unwrap().join("granted");
    std::fs::create_dir_all(&granted).unwrap();
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    let file = home.workspace_file();
    let raw_id = "0192c1a0-0000-7000-8000-00000000d001";
    let id = GrantId::new(raw_id);
    let row = grant(
        raw_id,
        write_root(&granted.to_string_lossy()),
        GrantScope::Workspace {
            root: home.ws.clone(),
        },
        Expiry::Never,
    );
    let allowed = |store: &GrantStore| {
        policy_of(&home, store).would_allow(&Blocked::FsWrite {
            path: granted.join("out.txt"),
        })
    };
    let on_disk = |text: &str| text.contains(raw_id);

    let holder = hold_lock(&file);
    start_then_drop(store.add(row)).await;
    assert!(store.live().is_empty(), "not on disk yet, so not in memory");
    assert!(!allowed(&store));
    drop(holder);
    until_file(&file, on_disk).await;
    assert_eq!(vec![id.clone()], ids_of(&store.live()), "landed, so live");
    assert!(allowed(&store));
    assert!(
        !store.reload_if_changed().await,
        "memory already is the file"
    );

    let holder = hold_lock(&file);
    start_then_drop(store.revoke(&id)).await;
    assert_eq!(
        vec![id.clone()],
        ids_of(&store.live()),
        "not revoked on disk yet"
    );
    drop(holder);
    until_file(&file, |text| !on_disk(text)).await;
    assert!(
        store.live().is_empty(),
        "revoked on disk, so gone from memory"
    );
    assert!(!allowed(&store));
    assert!(
        !store.reload_if_changed().await,
        "memory already is the file"
    );
    store
        .add(grant(
            "0192c1a0-0000-7000-8000-00000000d002",
            write_root("/opt/fixture/elsewhere/next"),
            GrantScope::Global,
            Expiry::Never,
        ))
        .await
        .unwrap();
    assert!(!ids_of(&store.live()).contains(&id), "stays revoked");
    assert!(!allowed(&store));
    assert_eq!(
        ids_of(&home.open(clock).await.live()),
        ids_of(&store.live())
    );
}

fn ids_of(rows: &[Grant]) -> Vec<GrantId> {
    rows.iter().map(|g| g.id.clone()).collect()
}

#[test]
fn fixture_round_trips_and_pins_the_file_shape() {
    let file: GrantFile = toml::from_str(FIXTURE).expect("fixture parses");
    assert_eq!(3, file.grants.len());
    let first = file.grants.first().unwrap();
    assert_eq!(
        write_root("/opt/ws-fixture/s/proj/node_modules"),
        first.subject
    );
    assert_eq!(
        GrantScope::Workspace {
            root: PathBuf::from("/opt/ws-fixture/s/proj")
        },
        first.scope
    );
    assert_eq!(Expiry::Ttl { seconds: 86400 }, first.expires);
    assert_eq!(GrantDecision::Allow, first.decision);
    assert_eq!(
        Some("npm ci"),
        first.via.as_ref().map(|v| v.command.as_str())
    );
    let third = file.grants.get(2).unwrap();
    assert_eq!(GrantDecision::Deny, third.decision);
    assert_eq!(Expiry::Never, third.expires);
    let text = toml::to_string_pretty(&file).unwrap();
    let back: GrantFile = toml::from_str(&text).unwrap();
    assert_eq!(file.grants, back.grants);
    assert!(text.contains("[[grant]]"));
}

#[tokio::test]
async fn workspace_grant_is_persisted_owner_only_and_reloaded_by_a_fresh_store() {
    let home = Home::new("persist");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    let root = home.ws.join("node_modules");
    let id = store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000010",
            GrantSubject::FsWriteRoot { root: root.clone() },
            GrantScope::Workspace {
                root: home.ws.clone(),
            },
            Expiry::Ttl { seconds: 3600 },
        ))
        .await
        .unwrap();
    assert_eq!(home.workspace_file(), store.workspace_file());
    assert!(home.workspace_file().is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.workspace_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(0o600, mode);
    }
    let reopened = home.open(clock).await;
    let live = reopened.live();
    assert_eq!(1, live.len());
    let only = live.first().unwrap();
    assert_eq!(id, only.id);
    // The subject is canonicalised at `add`, so a `/var` spelling comes back as `/private/var`
    // on macOS.
    assert_eq!(
        GrantSubject::FsWriteRoot {
            root: canonical_path(&root)
        },
        only.subject
    );
}

/// A session row belongs to the hub session that gave it and goes with it;
/// call rows never enter the store and a session row never goes through `add`.
#[tokio::test]
async fn session_rows_are_keyed_by_session_and_call_rows_are_refused() {
    let home = Home::new("session");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    store
        .add_session(
            "s-1",
            grant(
                "0192c1a0-0000-7000-8000-000000000011",
                write_root("/opt/fixture/elsewhere/a"),
                GrantScope::Session,
                Expiry::Never,
            ),
        )
        .unwrap();
    store
        .add_session(
            "s-2",
            grant(
                "0192c1a0-0000-7000-8000-000000000013",
                write_root("/opt/fixture/elsewhere/c"),
                GrantScope::Session,
                Expiry::Never,
            ),
        )
        .unwrap();
    let err = store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000012",
            write_root("/opt/fixture/elsewhere/b"),
            GrantScope::Call,
            Expiry::Never,
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, GrantError::CallScoped), "{err}");
    let err = store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000014",
            write_root("/opt/fixture/elsewhere/d"),
            GrantScope::Session,
            Expiry::Never,
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, GrantError::SessionScoped), "{err}");
    let err = store
        .add_session(
            "s-1",
            grant(
                "0192c1a0-0000-7000-8000-000000000015",
                write_root("/opt/fixture/elsewhere/e"),
                GrantScope::Global,
                Expiry::Never,
            ),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            GrantError::NotSessionScoped {
                scope: GrantScope::Global
            }
        ),
        "{err}"
    );
    assert_eq!(2, store.live().len());
    assert!(
        store.live_shared().is_empty(),
        "session rows are not shared"
    );
    let owners: Vec<(String, GrantSubject)> = store
        .live_session_rows()
        .into_iter()
        .map(|(session, grant)| (session, grant.subject))
        .collect();
    assert_eq!(
        vec![
            ("s-1".to_owned(), write_root("/opt/fixture/elsewhere/a")),
            ("s-2".to_owned(), write_root("/opt/fixture/elsewhere/c")),
        ],
        owners
    );
    assert!(!home.workspace_file().exists());
    assert!(!home.grok_home.join(GLOBAL_GRANTS_FILENAME).exists());

    assert_eq!(0, store.clear_session("s-9"));
    assert_eq!(1, store.clear_session("s-1"));
    let live = store.live();
    assert_eq!(1, live.len());
    assert_eq!(
        write_root("/opt/fixture/elsewhere/c"),
        live.first().unwrap().subject
    );
    store
        .revoke(&GrantId::new("0192c1a0-0000-7000-8000-000000000013"))
        .await
        .unwrap();
    assert!(store.live().is_empty());
}

/// Revoking against grant files that do not exist is `NotFound` and creates nothing: no session
/// directory, no grant file and no lock sidecar, here or in the grok home.
#[tokio::test]
async fn a_revoke_with_no_grant_file_creates_nothing() {
    let home = Home::new("revoke-missing");
    let mut store = home.open(Arc::new(FixedClock::at(T0))).await;
    let id = GrantId::new("0192c1a0-0000-7000-8000-0000000000e1");
    let err = store.revoke(&id).await.unwrap_err();
    assert!(matches!(err, GrantError::NotFound { .. }), "{err}");
    let session_dir = home.workspace_file().parent().unwrap().to_path_buf();
    assert!(!session_dir.exists(), "{session_dir:?}");
    let created: Vec<_> = std::fs::read_dir(&home.grok_home)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert!(created.is_empty(), "{created:?}");
}

/// The row enters memory only after the file is written.
#[tokio::test]
async fn failed_persist_leaves_memory_equal_to_disk() {
    let home = Home::new("persist-first");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock).await;
    // A file where the workspace directory should be makes every write fail
    let dir = home.workspace_file().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
    std::fs::write(&dir, "not a directory").unwrap();
    let err = store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000013",
            write_root("/opt/fixture/elsewhere/a"),
            GrantScope::Workspace {
                root: home.ws.clone(),
            },
            Expiry::Never,
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, GrantError::Io { .. }), "{err}");
    assert!(
        store.live().is_empty(),
        "the unwritten row is not in memory"
    );
}

/// Two stores on one file. A revoke in one and an add in the other must not
/// resurrect the revoked row, because every write re-reads the file first.
#[tokio::test]
async fn concurrent_writers_do_not_resurrect_a_revoked_row() {
    let home = Home::new("two-writers");
    let clock = Arc::new(FixedClock::at(T0));
    let mut a = home.open(clock.clone()).await;
    let wildcard = grant(
        "0192c1a0-0000-7000-8000-000000000014",
        GrantSubject::NetHost {
            host: HostPattern::all(),
            port: None,
        },
        GrantScope::Workspace {
            root: home.ws.clone(),
        },
        Expiry::Never,
    );
    a.add(wildcard.clone()).await.unwrap();
    let mut b = home.open(clock.clone()).await;
    assert_eq!(1, b.live().len());
    // The file signature must move even though this rewrite happens within the same second
    a.revoke(&wildcard.id).await.unwrap();
    b.add(grant(
        "0192c1a0-0000-7000-8000-000000000015",
        write_root("/opt/fixture/elsewhere/b"),
        GrantScope::Workspace {
            root: home.ws.clone(),
        },
        Expiry::Never,
    ))
    .await
    .unwrap();
    let fresh = home.open(clock).await;
    let ids: Vec<String> = fresh.live().iter().map(|g| g.id.to_string()).collect();
    assert_eq!(vec!["0192c1a0-0000-7000-8000-000000000015".to_owned()], ids);
    assert!(!b.live().iter().any(|g| g.id == wildcard.id));
}

/// A revoke of a row another writer already removed changes nothing on disk, and the store still
/// drops it from memory: it is not listed, or applied, until some later reload.
#[tokio::test]
async fn a_revoke_that_finds_the_row_gone_still_drops_it_from_memory() {
    let home = Home::new("revoke-gone");
    let clock = Arc::new(FixedClock::at(T0));
    let mut a = home.open(clock.clone()).await;
    let row = grant(
        "0192c1a0-0000-7000-8000-000000000016",
        write_root("/opt/fixture/elsewhere/gone"),
        GrantScope::Workspace {
            root: home.ws.clone(),
        },
        Expiry::Never,
    );
    a.add(row.clone()).await.unwrap();
    let mut b = home.open(clock).await;
    assert_eq!(1, b.live().len());
    a.revoke(&row.id).await.unwrap();
    let err = b.revoke(&row.id).await.unwrap_err();
    assert!(matches!(err, GrantError::NotFound { .. }), "{err}");
    assert!(b.live().is_empty(), "{:?}", b.live());
}

/// A FIFO in a grants file's place loads as empty and refuses edits without blocking the store,
/// and a workspace file whose session directory is a symlink is neither read nor written.
#[cfg(unix)]
#[tokio::test]
async fn a_fifo_or_a_redirected_session_directory_is_refused_without_blocking() {
    let home = Home::new("fifo");
    let clock = Arc::new(FixedClock::at(T0));
    let global = home.grok_home.join(GLOBAL_GRANTS_FILENAME);
    let made = std::process::Command::new("mkfifo")
        .arg(&global)
        .status()
        .unwrap();
    assert!(made.success());
    let unblock = || drop(std::fs::OpenOptions::new().write(true).open(&global));
    let opened = tokio::time::timeout(Duration::from_secs(10), home.open(clock.clone())).await;
    let Ok(mut store) = opened else {
        unblock();
        panic!("opening the store blocked on a FIFO");
    };
    assert!(store.live().is_empty());
    let add = store.add(grant(
        "0192c1a0-0000-7000-8000-000000000017",
        write_root("/opt/fixture/elsewhere/fifo"),
        GrantScope::Global,
        Expiry::Never,
    ));
    let Ok(added) = tokio::time::timeout(Duration::from_secs(10), add).await else {
        unblock();
        panic!("an edit blocked on a FIFO");
    };
    assert!(added.is_err(), "{added:?}");
    std::fs::remove_file(&global).unwrap();

    let session_dir = home.workspace_file().parent().unwrap().to_path_buf();
    let planted = home.ws.parent().unwrap().join("planted");
    std::fs::create_dir_all(&planted).unwrap();
    let forged = GrantFile {
        grants: vec![grant(
            "0192c1a0-0000-7000-8000-000000000018",
            write_root("/opt/fixture/elsewhere/forged"),
            GrantScope::Workspace {
                root: home.ws.clone(),
            },
            Expiry::Never,
        )],
    };
    std::fs::write(
        planted.join(GLOBAL_GRANTS_FILENAME),
        toml::to_string_pretty(&forged).unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(session_dir.parent().unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&session_dir);
    std::os::unix::fs::symlink(&planted, &session_dir).unwrap();
    let mut store = home.open(clock).await;
    assert!(store.live().is_empty(), "{:?}", store.live());
    let err = store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000019",
            write_root("/opt/fixture/elsewhere/ws"),
            GrantScope::Workspace {
                root: home.ws.clone(),
            },
            Expiry::Never,
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, GrantError::Io { .. }), "{err}");
}

/// A rows file, its lock and its replacement go through the directory handles the walk judged: a
/// session directory renamed away after the walk, with a symlink to a folder holding a forged
/// rows file left in its place, redirects none of them, and the next walk refuses the link.
#[cfg(unix)]
#[test]
fn a_directory_swapped_after_the_walk_redirects_neither_the_read_nor_the_lock() {
    let home = Home::new("swapped-after-walk");
    let file = home.workspace_file();
    let (name, lock) = (file.file_name().unwrap(), lock_path(&file));
    let lock_name = lock.file_name().unwrap();
    let session_dir = file.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(&file, "# judged\n").unwrap();
    let dir = rows_dir(&home.grok_home, &file, false).unwrap();

    let planted = home.ws.parent().unwrap().join("planted-after-walk");
    std::fs::create_dir_all(&planted).unwrap();
    std::fs::write(planted.join(name), "# forged\n").unwrap();
    let away = session_dir.with_file_name("moved-away");
    std::fs::rename(&session_dir, &away).unwrap();
    std::os::unix::fs::symlink(&planted, &session_dir).unwrap();

    let read = |dir: &HeldDir| dir.read(name, MAX_GRANTS_FILE_BYTES, FileOwner::Daemon);
    assert_eq!("# judged\n", read(&dir).unwrap());
    drop(dir.open_lock(lock_name).unwrap());
    assert!(
        away.join(lock_name).exists(),
        "the lock is in the judged directory"
    );
    assert!(
        !planted.join(lock_name).exists(),
        "the lock followed the link"
    );
    dir.replace(name, "# replaced\n", FileOwner::Daemon)
        .unwrap();
    assert_eq!("# replaced\n", read(&dir).unwrap());
    assert_eq!(
        "# forged\n",
        std::fs::read_to_string(planted.join(name)).unwrap()
    );
    let error = rows_dir(&home.grok_home, &file, true)
        .map(|_| ())
        .unwrap_err();
    assert_eq!(std::io::ErrorKind::InvalidInput, error.kind(), "{error}");
    assert!(error.to_string().contains("is a symlink"), "{error}");
}

/// Grants files a command left as symlinks while the folder was `off`, into a folder it may still
/// write once the folder enforces, are never followed: the reload after the flip loads neither
/// target's rows, nor does a fresh store; an add is refused and the target is never written; the
/// persist itself replaces a link standing at the path rather than writing through it. A grok
/// home that is itself a symlink holds the global file no more than a redirected session
/// directory holds the workspace file.
#[cfg(unix)]
#[tokio::test]
async fn a_grants_file_planted_as_a_symlink_is_neither_loaded_nor_written_through() {
    let home = Home::new("planted-link");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    assert!(store.live().is_empty());

    let target = home.ws.join("forged.toml");
    let forged = GrantFile {
        grants: vec![grant(
            "0192c1a0-0000-7000-8000-000000000070",
            write_root("/opt/fixture/elsewhere/forged"),
            GrantScope::Global,
            Expiry::Never,
        )],
    };
    std::fs::write(&target, toml::to_string_pretty(&forged).unwrap()).unwrap();
    let before = std::fs::read(&target).unwrap();
    let workspace_file = home.workspace_file();
    std::fs::create_dir_all(workspace_file.parent().unwrap()).unwrap();
    let global_file = home.grok_home.join(GLOBAL_GRANTS_FILENAME);
    for link in [&workspace_file, &global_file] {
        std::os::unix::fs::symlink(&target, link).unwrap();
    }

    assert!(store.reload_if_changed().await);
    assert!(store.live().is_empty(), "{:?}", store.live());
    assert!(home.open(clock.clone()).await.live().is_empty());
    for scope in [
        GrantScope::Workspace {
            root: home.ws.clone(),
        },
        GrantScope::Global,
    ] {
        let err = store
            .add(grant(
                "0192c1a0-0000-7000-8000-000000000071",
                write_root("/opt/fixture/elsewhere/added"),
                scope,
                Expiry::Never,
            ))
            .await
            .unwrap_err();
        assert!(matches!(err, GrantError::Unreadable { .. }), "{err}");
    }
    assert_eq!(before, std::fs::read(&target).unwrap(), "target written");

    rows_dir(&home.grok_home, &global_file, false)
        .unwrap()
        .replace(file_name(&global_file), "", FileOwner::Daemon)
        .unwrap();
    assert!(std::fs::symlink_metadata(&global_file).unwrap().is_file());
    assert_eq!(
        before,
        std::fs::read(&target).unwrap(),
        "persist wrote through the link"
    );

    let real_home = home.ws.parent().unwrap().join("real-grok");
    std::fs::create_dir_all(&real_home).unwrap();
    std::fs::write(
        real_home.join(GLOBAL_GRANTS_FILENAME),
        toml::to_string_pretty(&forged).unwrap(),
    )
    .unwrap();
    let linked_home = home.ws.parent().unwrap().join("linked-grok");
    std::os::unix::fs::symlink(&real_home, &linked_home).unwrap();
    let open_at = |grok_home: PathBuf| {
        let (clock, protected) = (clock.clone(), home.protected());
        let (ws, user_home) = (home.ws.clone(), home.user_home.clone());
        async move { GrantStore::open_in(&grok_home, &ws, protected, Some(&user_home), clock).await }
    };
    assert_eq!(1, open_at(real_home).await.live().len(), "control");
    assert!(open_at(linked_home).await.live().is_empty());
}

/// A rows file (or a link) swapped in under the temp's name between the write and the rename
/// lands in the rows file's place: the persist is refused and removes it, so no load reads it.
#[cfg(unix)]
#[tokio::test]
async fn a_rows_file_swapped_in_under_the_temp_name_is_refused_and_removed() {
    let home = Home::new("swapped-temp");
    let clock = Arc::new(FixedClock::at(T0));
    let global_file = home.grok_home.join(GLOBAL_GRANTS_FILENAME);
    let forged = GrantFile {
        grants: vec![grant(
            "0192c1a0-0000-7000-8000-000000000072",
            write_root("/opt/fixture/elsewhere/forged"),
            GrantScope::Global,
            Expiry::Never,
        )],
    };
    let staged = home.ws.join("forged.toml");
    std::fs::write(&staged, toml::to_string_pretty(&forged).unwrap()).unwrap();
    let dir = rows_dir(&home.grok_home, &global_file, true).unwrap();
    dir.replace(file_name(&global_file), "", FileOwner::Daemon)
        .unwrap();
    for as_link in [false, true] {
        let (grok_home, staged) = (home.grok_home.clone(), staged.clone());
        let swaps = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = std::rc::Rc::clone(&swaps);
        protected::BEFORE_HELD_RENAME.set(Some(Box::new(move |temp: &std::ffi::OsStr| {
            counted.set(counted.get() + 1);
            let swapped = grok_home.join("swapped");
            if as_link {
                std::os::unix::fs::symlink(&staged, &swapped).unwrap();
            } else {
                std::fs::copy(&staged, &swapped).unwrap();
            }
            std::fs::rename(&swapped, grok_home.join(temp)).unwrap();
        })));
        let refused = dir.replace(file_name(&global_file), "", FileOwner::Daemon);
        protected::BEFORE_HELD_RENAME.set(None);
        let error = refused.unwrap_err();
        assert_eq!(std::io::ErrorKind::InvalidInput, error.kind(), "{error}");
        assert_eq!(1, swaps.get());
        assert!(
            std::fs::symlink_metadata(&global_file).is_err(),
            "the swapped-in file is gone (link: {as_link})"
        );
        assert!(home.open(clock.clone()).await.live().is_empty());
        let names: Vec<_> = std::fs::read_dir(&home.grok_home)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert!(
            !names.iter().any(|n| n.to_string_lossy().ends_with(".tmp")),
            "{names:?}"
        );
    }
}

/// Two stores on one file (a second served folder
/// sharing the global file, another daemon) add rows at the same time and none is lost — each
/// read-modify-write runs under the file's lock, so the interleaving B-sb-4 found (2–14 of 20
/// rounds lost a row) cannot happen. Every add goes to the same global file from a separate
/// store, all at once on the multi-thread runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_adds_from_two_stores_on_one_file_lose_no_row() {
    let home = Home::new("contended");
    let clock = Arc::new(FixedClock::at(T0));
    const WRITERS: usize = 2;
    const ROUNDS: usize = 12;
    let mut tasks = tokio::task::JoinSet::new();
    for writer in 0..WRITERS {
        let mut store = home.open(clock.clone()).await;
        tasks.spawn(async move {
            for round in 0..ROUNDS {
                store
                    .add(grant(
                        &format!("0192c1a0-0000-7000-8000-0000000{writer:02}{round:04}"),
                        write_root(&format!("/opt/fixture/elsewhere/w{writer}/r{round}")),
                        GrantScope::Global,
                        Expiry::Never,
                    ))
                    .await
                    .unwrap();
            }
            store
        });
    }
    let mut stores = Vec::new();
    while let Some(store) = tasks.join_next().await {
        stores.push(store.unwrap());
    }
    let fresh = home.open(clock).await;
    let mut ids: Vec<String> = fresh.live().iter().map(|g| g.id.to_string()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        WRITERS * ROUNDS,
        ids.len(),
        "every row of every writer is on disk: {ids:?}"
    );
    for store in &mut stores {
        let held = store.live().len();
        assert!(
            (ROUNDS..=WRITERS * ROUNDS).contains(&held),
            "a writer's memory is the file as of its own last write, never less than its own rows: {held}"
        );
        store.reload_if_changed().await;
        assert_eq!(WRITERS * ROUNDS, store.live().len());
    }
    assert!(
        lock_path(fresh.global_file()).exists(),
        "the writers took the sidecar lock beside the file"
    );
}

#[tokio::test]
async fn future_granted_at_is_clamped_at_load_and_an_unknown_subject_drops_the_file() {
    let home = Home::new("clamp");
    let path = home.workspace_file();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut future = grant(
        "0192c1a0-0000-7000-8000-000000000016",
        write_root("/opt/fixture/elsewhere/a"),
        GrantScope::Workspace {
            root: home.ws.clone(),
        },
        Expiry::Ttl { seconds: 60 },
    );
    future.granted_at = T0 + 1_000_000;
    let file = GrantFile {
        grants: vec![future],
    };
    std::fs::write(&path, toml::to_string(&file).unwrap()).unwrap();
    let clock = Arc::new(FixedClock::at(T0));
    let store = home.open(clock.clone()).await;
    let live = store.live();
    assert_eq!(1, live.len());
    assert_eq!(T0, live.first().unwrap().granted_at, "clamped to now");
    clock.advance(60);
    assert!(
        store.live().is_empty(),
        "the TTL runs from now, not from the future"
    );

    // A subject kind this version does not know (`cmd_prefix`) fails the parse and drops the
    // file whole: the store never holds a row it cannot read
    let stale = format!(
        "{}\n[[grant]]\nid = \"0192c1a0-0000-7000-8000-000000000017\"\nsubject = {{ kind = \"cmd_prefix\", argv = [\"npm\"] }}\nscope = {{ kind = \"global\" }}\nexpires = {{ kind = \"never\" }}\ngranted_at = {T0}\ngranted_by = \"cli\"\n",
        toml::to_string(&GrantFile {
            grants: vec![grant(
                "0192c1a0-0000-7000-8000-000000000018",
                write_root("/opt/fixture/elsewhere/b"),
                GrantScope::Workspace {
                    root: home.ws.clone(),
                },
                Expiry::Never,
            )],
        })
        .unwrap()
    );
    std::fs::write(&path, stale).unwrap();
    let store = home.open(Arc::new(FixedClock::at(T0))).await;
    assert!(store.live().is_empty(), "{:?}", store.live());
}

#[tokio::test]
async fn expired_rows_are_invisible_and_dropped_on_the_next_persist() {
    let home = Home::new("expiry");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000020",
            write_root("/opt/fixture/elsewhere/short"),
            GrantScope::Global,
            Expiry::Ttl { seconds: 60 },
        ))
        .await
        .unwrap();
    store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000021",
            write_root("/opt/fixture/elsewhere/long"),
            GrantScope::Global,
            Expiry::At {
                unix: T0 + 7 * 86400,
            },
        ))
        .await
        .unwrap();
    assert_eq!(2, store.live().len());
    clock.advance(60);
    let live = store.live();
    assert_eq!(1, live.len());
    assert_eq!(
        write_root("/opt/fixture/elsewhere/long"),
        live.first().unwrap().subject
    );
    let on_disk = std::fs::read_to_string(home.grok_home.join(GLOBAL_GRANTS_FILENAME)).unwrap();
    assert!(
        on_disk.contains("/opt/fixture/elsewhere/short"),
        "expired row lingers until the next persist"
    );
    store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000022",
            write_root("/opt/fixture/elsewhere/third"),
            GrantScope::Global,
            Expiry::Never,
        ))
        .await
        .unwrap();
    let on_disk = std::fs::read_to_string(home.grok_home.join(GLOBAL_GRANTS_FILENAME)).unwrap();
    assert!(!on_disk.contains("/opt/fixture/elsewhere/short"));
    assert!(on_disk.contains("/opt/fixture/elsewhere/long"));
    assert!(on_disk.contains("/opt/fixture/elsewhere/third"));
}

#[tokio::test]
async fn protected_rows_are_refused_on_add_and_dropped_at_load() {
    let home = Home::new("protected");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    let err = store
        .add_session(
            "s-1",
            grant(
                "0192c1a0-0000-7000-8000-000000000030",
                GrantSubject::FsWriteRoot {
                    root: home.ws.join(".git/hooks"),
                },
                GrantScope::Session,
                Expiry::Never,
            ),
        )
        .unwrap_err();
    assert!(matches!(err, GrantError::Protected { .. }), "{err}");

    // A file edited by hand to allow the floor loads without that row
    let forged = GrantFile {
        grants: vec![
            grant(
                "0192c1a0-0000-7000-8000-000000000031",
                GrantSubject::FsWriteRoot {
                    root: home.grok_home.join("hooks"),
                },
                GrantScope::Global,
                Expiry::Never,
            ),
            grant(
                "0192c1a0-0000-7000-8000-000000000032",
                write_root("/opt/fixture/elsewhere/fine"),
                GrantScope::Global,
                Expiry::Never,
            ),
            // A `..`-spelled row is canonicalised at load before the floor is asked …
            grant(
                "0192c1a0-0000-7000-8000-000000000033",
                GrantSubject::FsWriteRoot {
                    root: home.grok_home.join("sessions/../hooks-paths"),
                },
                GrantScope::Global,
                Expiry::Never,
            ),
            // … and one outside the floor loads in its folded spelling
            grant(
                "0192c1a0-0000-7000-8000-000000000034",
                write_root("/opt/fixture/elsewhere/x/../also-fine/."),
                GrantScope::Global,
                Expiry::Never,
            ),
        ],
    };
    std::fs::write(
        home.grok_home.join(GLOBAL_GRANTS_FILENAME),
        toml::to_string_pretty(&forged).unwrap(),
    )
    .unwrap();
    let store = home.open(clock).await;
    let live = store.live();
    let subjects: Vec<&GrantSubject> = live.iter().map(|g| &g.subject).collect();
    assert_eq!(
        vec![
            &write_root("/opt/fixture/elsewhere/fine"),
            &write_root("/opt/fixture/elsewhere/also-fine")
        ],
        subjects
    );
}

/// The floor names submodule hooks by pattern (`<ws>/.git/modules/**/hooks`): a row naming a
/// match, or a folder beneath the pattern's literal prefix that could hold one, is refused on
/// add and dropped at load like a row naming a listed path.
#[tokio::test]
async fn rows_the_floor_glob_reaches_are_refused_on_add_and_dropped_at_load() {
    let home = Home::new("protected-glob");
    std::fs::create_dir_all(home.ws.join(".git/modules/x/hooks")).unwrap();
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    let submodule = home.ws.join(".git/modules/x");
    let err = store
        .add_session(
            "s-1",
            grant(
                "0192c1a0-0000-7000-8000-000000000040",
                GrantSubject::FsWriteRoot {
                    root: submodule.clone(),
                },
                GrantScope::Session,
                Expiry::Never,
            ),
        )
        .unwrap_err();
    assert!(matches!(err, GrantError::Protected { .. }), "{err}");

    let forged = GrantFile {
        grants: vec![
            grant(
                "0192c1a0-0000-7000-8000-000000000041",
                GrantSubject::FsWriteRoot {
                    root: submodule.join("hooks"),
                },
                GrantScope::Global,
                Expiry::Never,
            ),
            grant(
                "0192c1a0-0000-7000-8000-000000000042",
                GrantSubject::FsWriteRoot { root: submodule },
                GrantScope::Global,
                Expiry::Never,
            ),
            grant(
                "0192c1a0-0000-7000-8000-000000000043",
                write_root("/opt/fixture/elsewhere/fine"),
                GrantScope::Global,
                Expiry::Never,
            ),
        ],
    };
    std::fs::write(
        home.grok_home.join(GLOBAL_GRANTS_FILENAME),
        toml::to_string_pretty(&forged).unwrap(),
    )
    .unwrap();
    let store = home.open(clock).await;
    let subjects: Vec<GrantSubject> = store.live().into_iter().map(|g| g.subject).collect();
    assert_eq!(vec![write_root("/opt/fixture/elsewhere/fine")], subjects);
}

/// A relative path subject can never be applied by a policy, so it is refused on add and a
/// hand-edited row is dropped at load instead of refusing every later policy build.
#[tokio::test]
async fn relative_path_rows_are_refused_on_add_and_dropped_at_load() {
    let home = Home::new("relative");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    let relative = GrantSubject::FsRead {
        root: PathBuf::from("build/out"),
    };
    let err = store.check_subject(&relative).unwrap_err();
    assert!(matches!(err, GrantError::NotAbsolute { .. }), "{err}");
    let err = store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000035",
            relative.clone(),
            GrantScope::Global,
            Expiry::Never,
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, GrantError::NotAbsolute { .. }), "{err}");

    let edited = GrantFile {
        grants: vec![
            grant(
                "0192c1a0-0000-7000-8000-000000000036",
                relative,
                GrantScope::Global,
                Expiry::Never,
            ),
            grant(
                "0192c1a0-0000-7000-8000-000000000037",
                write_root("/opt/fixture/elsewhere/fine"),
                GrantScope::Global,
                Expiry::Never,
            ),
        ],
    };
    std::fs::write(
        home.grok_home.join(GLOBAL_GRANTS_FILENAME),
        toml::to_string_pretty(&edited).unwrap(),
    )
    .unwrap();
    let store = home.open(clock).await;
    let subjects: Vec<GrantSubject> = store.live().into_iter().map(|g| g.subject).collect();
    assert_eq!(vec![write_root("/opt/fixture/elsewhere/fine")], subjects);
}

/// A row is held to the card's rules whatever sent it: a root spelled through a symlink below
/// its top-level component (`<ws>/out -> /`) or one too broad for any card (`/`, `/tmp`, the
/// home the store was opened with) is refused on add and for a session, and dropped at load — as
/// spelled on disk, so a granted folder since swapped for a symlink to an unrelated one is not
/// followed there.
#[cfg(unix)]
#[tokio::test]
async fn symlinked_and_too_broad_roots_are_refused_on_add_and_dropped_at_load() {
    let home = Home::new("broad");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    let out = home.ws.join("out");
    std::os::unix::fs::symlink("/", &out).unwrap();
    let swapped_target = home.ws.parent().unwrap().join("unrelated/deep/folder");
    std::fs::create_dir_all(&swapped_target).unwrap();
    let swapped = home.ws.join("build");
    std::os::unix::fs::symlink(&swapped_target, &swapped).unwrap();
    for root in [out.clone(), out.join("sub")] {
        let subject = GrantSubject::FsWriteRoot { root };
        let err = store.check_subject(&subject).unwrap_err();
        assert!(matches!(err, GrantError::Root(_)), "{err}");
        let session = grant(
            "0192c1a0-0000-7000-8000-000000000038",
            subject.clone(),
            GrantScope::Session,
            Expiry::Never,
        );
        let err = store.add_session("s-1", session).unwrap_err();
        assert!(matches!(err, GrantError::Root(_)), "{err}");
        let global = grant(
            "0192c1a0-0000-7000-8000-00000000003e",
            subject,
            GrantScope::Global,
            Expiry::Never,
        );
        let err = store.add(global).await.unwrap_err();
        assert!(matches!(err, GrantError::Root(_)), "{err}");
    }
    let broad = ["/", "/tmp", "/Users"]
        .map(PathBuf::from)
        .into_iter()
        .chain([home.user_home.clone()]);
    for root in broad {
        let err = store
            .add(grant(
                "0192c1a0-0000-7000-8000-000000000039",
                GrantSubject::FsRead { root: root.clone() },
                GrantScope::Global,
                Expiry::Never,
            ))
            .await
            .unwrap_err();
        assert!(
            matches!(err, GrantError::TooBroad { .. }),
            "{root:?}: {err}"
        );
    }
    assert!(store.live().is_empty());

    let edited = GrantFile {
        grants: vec![
            grant(
                "0192c1a0-0000-7000-8000-00000000003a",
                write_root("/"),
                GrantScope::Global,
                Expiry::Never,
            ),
            grant(
                "0192c1a0-0000-7000-8000-00000000003b",
                GrantSubject::FsWriteRoot { root: out },
                GrantScope::Global,
                Expiry::Never,
            ),
            grant(
                "0192c1a0-0000-7000-8000-00000000003d",
                GrantSubject::FsWriteRoot { root: swapped },
                GrantScope::Global,
                Expiry::Never,
            ),
            grant(
                "0192c1a0-0000-7000-8000-00000000003f",
                GrantSubject::FsRead {
                    root: home.user_home.clone(),
                },
                GrantScope::Global,
                Expiry::Never,
            ),
            grant(
                "0192c1a0-0000-7000-8000-00000000003c",
                write_root("/opt/fixture/elsewhere/fine"),
                GrantScope::Global,
                Expiry::Never,
            ),
        ],
    };
    std::fs::write(
        home.grok_home.join(GLOBAL_GRANTS_FILENAME),
        toml::to_string_pretty(&edited).unwrap(),
    )
    .unwrap();
    let store = home.open(clock).await;
    let subjects: Vec<GrantSubject> = store.live().into_iter().map(|g| g.subject).collect();
    assert_eq!(vec![write_root("/opt/fixture/elsewhere/fine")], subjects);
}

#[tokio::test]
async fn deny_rows_win_in_live_allows() {
    let home = Home::new("deny");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock).await;
    store
        .add_session(
            "s-1",
            grant(
                "0192c1a0-0000-7000-8000-000000000040",
                write_root("/opt/fixture/elsewhere/tree"),
                GrantScope::Session,
                Expiry::Never,
            ),
        )
        .unwrap();
    store
        .add_session(
            "s-1",
            grant(
                "0192c1a0-0000-7000-8000-000000000041",
                GrantSubject::NetHost {
                    host: HostPattern::new("api.example.com"),
                    port: Some(443),
                },
                GrantScope::Session,
                Expiry::Never,
            ),
        )
        .unwrap();
    let mut deny = grant(
        "0192c1a0-0000-7000-8000-000000000042",
        write_root("/opt/fixture/elsewhere"),
        GrantScope::Session,
        Expiry::Never,
    );
    deny.decision = GrantDecision::Deny;
    store.add_session("s-1", deny).unwrap();
    let mut deny_net = grant(
        "0192c1a0-0000-7000-8000-000000000043",
        GrantSubject::NetHost {
            host: HostPattern::new("*.example.com"),
            port: None,
        },
        GrantScope::Session,
        Expiry::Never,
    );
    deny_net.decision = GrantDecision::Deny;
    store.add_session("s-1", deny_net).unwrap();
    assert_eq!(4, store.live().len());
    assert!(store.live_allows().is_empty());
}

/// A narrower path deny inside a broader allow takes the allow out: the policy cannot exclude a
/// subtree from one grant, so keeping the allow would leave the denied tree writable through it.
/// An allow beside the deny stays.
#[tokio::test]
async fn a_narrower_path_deny_takes_out_the_broader_allow_around_it() {
    let home = Home::new("deny-inside");
    let mut store = home.open(Arc::new(FixedClock::at(T0))).await;
    for (id, root) in [
        (
            "0192c1a0-0000-7000-8000-000000000050",
            "/opt/fixture/elsewhere/tree",
        ),
        (
            "0192c1a0-0000-7000-8000-000000000051",
            "/opt/fixture/elsewhere/beside",
        ),
    ] {
        store
            .add_session(
                "s-1",
                grant(id, write_root(root), GrantScope::Session, Expiry::Never),
            )
            .unwrap();
    }
    let mut deny = grant(
        "0192c1a0-0000-7000-8000-000000000052",
        write_root("/opt/fixture/elsewhere/tree/secret"),
        GrantScope::Session,
        Expiry::Never,
    );
    deny.decision = GrantDecision::Deny;
    store.add_session("s-1", deny).unwrap();
    let subjects: Vec<GrantSubject> = store
        .live_allows()
        .into_iter()
        .map(|grant| grant.subject)
        .collect();
    assert_eq!(vec![write_root("/opt/fixture/elsewhere/beside")], subjects);
}

/// A row dated ahead of the clock is clamped when it is recorded, as it is at load, so its TTL
/// runs from now: in memory, on disk, and for a session row.
#[tokio::test]
async fn a_future_granted_at_is_clamped_when_the_row_is_recorded() {
    let home = Home::new("future-dated");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    let mut row = grant(
        "0192c1a0-0000-7000-8000-000000000053",
        write_root("/opt/fixture/elsewhere/dated"),
        GrantScope::Global,
        Expiry::Ttl { seconds: 60 },
    );
    row.granted_at = T0 + 3_600;
    store.add(row).await.unwrap();
    let mut session_row = grant(
        "0192c1a0-0000-7000-8000-000000000054",
        write_root("/opt/fixture/elsewhere/dated-session"),
        GrantScope::Session,
        Expiry::Never,
    );
    session_row.granted_at = T0 + 3_600;
    store.add_session("s-1", session_row).unwrap();
    assert!(store.live().iter().all(|grant| grant.granted_at == T0));
    let fresh = home.open(clock).await;
    assert_eq!(
        vec![T0],
        fresh
            .live()
            .iter()
            .map(|g| g.granted_at)
            .collect::<Vec<_>>()
    );
}

/// The family grant persists as a workspace row and is shadowed whole by its own deny row (the
/// card's "Always reject") and by nothing else: a path deny row travels with it and closes only
/// the tree it falls in (`policy_tests`), so the family stays listed.
#[tokio::test]
async fn build_caches_row_persists_and_is_shadowed_only_by_a_build_caches_deny() {
    let home = Home::new("build-caches");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000061",
            GrantSubject::BuildCaches,
            GrantScope::Workspace {
                root: home.ws.clone(),
            },
            Expiry::Ttl {
                seconds: 7 * 86_400,
            },
        ))
        .await
        .unwrap();
    let reopened = home.open(clock).await;
    assert_eq!(
        vec![GrantSubject::BuildCaches],
        reopened
            .live_allows()
            .into_iter()
            .map(|g| g.subject)
            .collect::<Vec<_>>()
    );
    let mut path_deny = grant(
        "0192c1a0-0000-7000-8000-000000000062",
        write_root("/opt/ws-fixture/u/.cargo/registry"),
        GrantScope::Session,
        Expiry::Never,
    );
    path_deny.decision = GrantDecision::Deny;
    store.add_session("s-1", path_deny.clone()).unwrap();
    assert_eq!(1, store.live_allows().len(), "the family stays listed");
    let in_effect = allows_not_denied(&store.live());
    assert!(
        in_effect.contains(&path_deny),
        "the path deny travels with the family"
    );
    let mut deny = grant(
        "0192c1a0-0000-7000-8000-000000000063",
        GrantSubject::BuildCaches,
        GrantScope::Session,
        Expiry::Never,
    );
    deny.decision = GrantDecision::Deny;
    store.add_session("s-1", deny).unwrap();
    assert!(store.live_allows().is_empty());
}

/// A `*.example.com` deny row covers `api.example.com` but not
/// `example.com` itself nor `notexample.com`, exactly as the proxy matches patterns.
#[tokio::test]
async fn wildcard_deny_row_covers_subdomains_only() {
    let home = Home::new("deny-wildcard");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock).await;
    for (i, host) in ["api.example.com", "example.com", "notexample.com"]
        .into_iter()
        .enumerate()
    {
        store
            .add_session(
                "s-1",
                grant(
                    &format!("0192c1a0-0000-7000-8000-00000000005{i}"),
                    GrantSubject::NetHost {
                        host: HostPattern::new(host),
                        port: None,
                    },
                    GrantScope::Session,
                    Expiry::Never,
                ),
            )
            .unwrap();
    }
    let mut deny = grant(
        "0192c1a0-0000-7000-8000-000000000059",
        GrantSubject::NetHost {
            host: HostPattern::new("*.example.com"),
            port: None,
        },
        GrantScope::Session,
        Expiry::Never,
    );
    deny.decision = GrantDecision::Deny;
    store.add_session("s-1", deny).unwrap();
    let mut allowed: Vec<String> = store
        .live_allows()
        .iter()
        .map(|g| match &g.subject {
            GrantSubject::NetHost { host, .. } => host.to_string(),
            other => panic!("{other:?}"),
        })
        .collect();
    allowed.sort();
    assert_eq!(vec!["example.com", "notexample.com"], allowed);
}

/// A port deny under a host-wide allow travels with it, in the rows a policy and the decider are
/// built from: the host stays allowed, minus that port, whichever side names the wildcard. A deny
/// for another host stays out, and a deny covering the allow takes it out whole.
#[test]
fn a_port_deny_under_a_host_wide_allow_travels_with_it() {
    let net = |id: &str, host: &str, port: Option<u16>, decision: GrantDecision| Grant {
        decision,
        ..grant(
            id,
            GrantSubject::NetHost {
                host: HostPattern::new(host),
                port,
            },
            GrantScope::Session,
            Expiry::Never,
        )
    };
    let allow = net(
        "0192c1a0-0000-7000-8000-000000000080",
        "api.example.com",
        None,
        GrantDecision::Allow,
    );
    let port_deny = net(
        "0192c1a0-0000-7000-8000-000000000081",
        "api.example.com",
        Some(22),
        GrantDecision::Deny,
    );
    let wildcard_port_deny = net(
        "0192c1a0-0000-7000-8000-000000000082",
        "*.example.com",
        Some(25),
        GrantDecision::Deny,
    );
    let elsewhere = net(
        "0192c1a0-0000-7000-8000-000000000083",
        "other.example.org",
        Some(22),
        GrantDecision::Deny,
    );
    assert_eq!(
        vec![allow.clone(), port_deny.clone(), wildcard_port_deny.clone()],
        allows_not_denied(&[
            allow.clone(),
            port_deny.clone(),
            wildcard_port_deny,
            elsewhere,
        ])
    );
    let host_deny = net(
        "0192c1a0-0000-7000-8000-000000000084",
        "api.example.com",
        None,
        GrantDecision::Deny,
    );
    assert!(allows_not_denied(&[allow, port_deny, host_deny]).is_empty());
}

#[tokio::test]
async fn revoke_removes_the_row_and_rewrites_the_file() {
    let home = Home::new("revoke");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock).await;
    let id = store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000050",
            write_root("/opt/fixture/elsewhere/x"),
            GrantScope::Workspace {
                root: home.ws.clone(),
            },
            Expiry::Never,
        ))
        .await
        .unwrap();
    store.revoke(&id).await.unwrap();
    assert!(store.live().is_empty());
    let on_disk = std::fs::read_to_string(home.workspace_file()).unwrap();
    assert!(!on_disk.contains("/opt/fixture/elsewhere/x"));
    let err = store.revoke(&id).await.unwrap_err();
    assert!(matches!(err, GrantError::NotFound { .. }), "{err}");
}

#[tokio::test]
async fn external_edit_is_picked_up_by_reload_if_changed() {
    let home = Home::new("reload");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock).await;
    assert!(!store.reload_if_changed().await);
    let external = GrantFile {
        grants: vec![grant(
            "0192c1a0-0000-7000-8000-000000000060",
            write_root("/opt/fixture/elsewhere/from-desktop"),
            GrantScope::Global,
            Expiry::Never,
        )],
    };
    std::fs::write(
        home.grok_home.join(GLOBAL_GRANTS_FILENAME),
        toml::to_string_pretty(&external).unwrap(),
    )
    .unwrap();
    assert!(store.reload_if_changed().await);
    assert_eq!(1, store.live().len());
    assert!(!store.reload_if_changed().await);
}

#[tokio::test]
async fn unparsable_file_loads_as_empty() {
    let home = Home::new("garbage");
    std::fs::write(home.grok_home.join(GLOBAL_GRANTS_FILENAME), "not = [toml").unwrap();
    let store = home.open(Arc::new(FixedClock::at(T0))).await;
    assert!(store.live().is_empty());
}

/// A file the store cannot parse — garbage, or a newer writer's subject kind — loads as empty
/// and the next write fails instead of replacing it with the new row alone.
#[tokio::test]
async fn an_unparsable_file_is_never_overwritten_by_a_write() {
    let home = Home::new("no-wipe");
    let global = home.grok_home.join(GLOBAL_GRANTS_FILENAME);
    let unknown_kind = format!(
        "[[grant]]\nid = \"0192c1a0-0000-7000-8000-000000000070\"\nsubject = {{ kind = \"cmd_prefix\", argv = [\"npm\"] }}\nscope = {{ kind = \"global\" }}\nexpires = {{ kind = \"never\" }}\ngranted_at = {T0}\ngranted_by = \"cli\"\n"
    );
    for contents in ["not = [toml".to_owned(), unknown_kind] {
        std::fs::write(&global, &contents).unwrap();
        let mut store = home.open(Arc::new(FixedClock::at(T0))).await;
        assert!(store.live().is_empty());
        let err = store
            .add(grant(
                "0192c1a0-0000-7000-8000-000000000071",
                write_root("/opt/fixture/elsewhere/new"),
                GrantScope::Global,
                Expiry::Never,
            ))
            .await
            .unwrap_err();
        assert!(matches!(err, GrantError::Parse { .. }), "{err}");
        assert_eq!(contents, std::fs::read_to_string(&global).unwrap());
        assert!(
            store.live().is_empty(),
            "the unwritten row is not in memory"
        );
    }
}

/// A revoke still reaches the global file when the workspace file beside it is unparsable, leaves
/// that file as it found it, and an unknown id is still `NotFound` (the daemon's cue to try the
/// next folder).
#[tokio::test]
async fn revoke_reaches_the_global_file_past_an_unparsable_workspace_file() {
    let home = Home::new("revoke-past-garbage");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    let id = store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000072",
            write_root("/opt/fixture/elsewhere/global"),
            GrantScope::Global,
            Expiry::Never,
        ))
        .await
        .unwrap();
    let workspace = home.workspace_file();
    std::fs::create_dir_all(workspace.parent().unwrap()).unwrap();
    std::fs::write(&workspace, "not = [toml").unwrap();
    store.revoke(&id).await.unwrap();
    assert!(store.live().is_empty());
    assert_eq!("not = [toml", std::fs::read_to_string(&workspace).unwrap());
    let err = store.revoke(&id).await.unwrap_err();
    assert!(matches!(err, GrantError::NotFound { .. }), "{err}");
    assert_eq!("not = [toml", std::fs::read_to_string(&workspace).unwrap());
}

/// A file past the read cap is refused like an unreadable one: it loads as empty and is kept.
#[tokio::test]
async fn an_oversized_file_loads_as_empty_and_is_never_overwritten() {
    let home = Home::new("oversized");
    let global = home.grok_home.join(GLOBAL_GRANTS_FILENAME);
    let padding = "#".repeat(usize::try_from(MAX_GRANTS_FILE_BYTES).unwrap());
    let contents = format!("{padding}\n");
    std::fs::write(&global, &contents).unwrap();
    let mut store = home.open(Arc::new(FixedClock::at(T0))).await;
    assert!(store.live().is_empty());
    let err = store
        .add(grant(
            "0192c1a0-0000-7000-8000-000000000073",
            write_root("/opt/fixture/elsewhere/new"),
            GrantScope::Global,
            Expiry::Never,
        ))
        .await
        .unwrap_err();
    let GrantError::Unreadable { source, .. } = &err else {
        panic!("expected an unreadable file: {err}");
    };
    assert_eq!(std::io::ErrorKind::FileTooLarge, source.kind(), "{err}");
    assert_eq!(
        contents.len(),
        std::fs::read_to_string(&global).unwrap().len()
    );
}

/// While the workspace file cannot be read (here, past the read cap) the deny rows it held are
/// unknown, so the global file's allow is not shared, at open or after a reload, though the
/// listing still shows it; once the file loads again the allow is back.
#[tokio::test]
async fn an_unreadable_file_withholds_every_shared_allow_until_it_loads() {
    let home = Home::new("withhold");
    let clock = Arc::new(FixedClock::at(T0));
    let mut store = home.open(clock.clone()).await;
    let allow = grant(
        "0192c1a0-0000-7000-8000-0000000000a0",
        write_root("/opt/fixture/elsewhere/allowed"),
        GrantScope::Global,
        Expiry::Never,
    );
    let mut deny = grant(
        "0192c1a0-0000-7000-8000-0000000000a1",
        write_root("/opt/fixture/elsewhere/denied"),
        GrantScope::Global,
        Expiry::Never,
    );
    deny.decision = GrantDecision::Deny;
    store.add(allow.clone()).await.unwrap();
    store.add(deny.clone()).await.unwrap();
    let both = ids_of(&[allow.clone(), deny.clone()]);
    assert_eq!(both, ids_of(&store.live_shared()));

    let workspace = home.workspace_file();
    std::fs::create_dir_all(workspace.parent().unwrap()).unwrap();
    let oversized = "#".repeat(usize::try_from(MAX_GRANTS_FILE_BYTES).unwrap() + 1);
    std::fs::write(&workspace, &oversized).unwrap();
    let mut store = home.open(clock).await;
    assert_eq!(
        ids_of(std::slice::from_ref(&deny)),
        ids_of(&store.live_shared())
    );
    assert_eq!(both, ids_of(&store.live()));

    std::fs::remove_file(&workspace).unwrap();
    assert!(store.reload_if_changed().await);
    assert_eq!(both, ids_of(&store.live_shared()));
    std::fs::write(&workspace, &oversized).unwrap();
    assert!(store.reload_if_changed().await);
    assert_eq!(
        ids_of(std::slice::from_ref(&deny)),
        ids_of(&store.live_shared())
    );
}

/// A reader never waits on the lock a commit holds across its file I/O: with that lock taken
/// elsewhere (a write stalled on a slow disk, its caller gone), every listing returns at once
/// with the rows last committed.
#[tokio::test]
async fn readers_never_wait_on_a_commit_in_progress() {
    let home = Home::new("readers");
    let mut store = home.open(Arc::new(FixedClock::at(T0))).await;
    store
        .add(grant(
            "0192c1a0-0000-7000-8000-0000000000a2",
            write_root("/opt/fixture/elsewhere/tree"),
            GrantScope::Global,
            Expiry::Never,
        ))
        .await
        .unwrap();
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let (read_tx, read_rx) = std::sync::mpsc::channel();
    let signature = Arc::clone(&store.global.commit.signature);
    let read = std::thread::scope(|scope| {
        scope.spawn(move || {
            let _held = lock_value(&signature);
            locked_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        locked_rx.recv().unwrap();
        let store = &store;
        scope.spawn(move || {
            let lens = (store.live().len(), store.live_shared().len());
            let _ = read_tx.send(lens);
        });
        let read = read_rx.recv_timeout(Duration::from_secs(5));
        release_tx.send(()).unwrap();
        read
    });
    assert_eq!(Ok((1, 1)), read);
}
