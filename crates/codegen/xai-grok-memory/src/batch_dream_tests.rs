use super::*;
use crate::batch_dream_io::hash_bytes;
use tempfile::TempDir;

const LEASE: Duration = Duration::from_secs(60);

struct Fixture {
    _temp: TempDir,
    workspace: PathBuf,
    global: PathBuf,
    store: BatchDreamStore,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = TempDir::new().unwrap();
        let global = temp.path().join("global");
        let workspace = temp.path().join("workspace");
        crate::ensure_scope_initialized(temp.path(), &global, V2MemoryScope::Global).unwrap();
        crate::ensure_scope_initialized(temp.path(), &workspace, V2MemoryScope::Workspace).unwrap();
        let store =
            BatchDreamStore::open(&workspace, V2MemoryScope::Workspace, &global, &workspace)
                .unwrap();
        Fixture {
            _temp: temp,
            workspace,
            global,
            store,
        }
    }

    fn note(&self, name: &str, body: &str, modified: i64) -> String {
        let relative = format!("{INBOX_DIR}/{name}.md");
        let path = self.workspace.join(&relative);
        std::fs::write(&path, body).unwrap();
        filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(modified, 0)).unwrap();
        relative
    }

    fn topic(&self, name: &str, body: &str) -> String {
        let relative = format!("{TOPICS_DIR}/{name}.md");
        std::fs::write(self.workspace.join(&relative), body).unwrap();
        relative
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.workspace.join(relative)).unwrap()
    }

    fn claim(&self, now: i64, max_note_bytes: usize, excluded: &[String]) -> Option<BatchLease> {
        self.store
            .claim(&BatchClaimRequest {
                owner: "test",
                now,
                duration: LEASE,
                max_note_bytes,
                excluded,
            })
            .unwrap()
    }

    fn state(&self) -> rusqlite::Connection {
        self.store.connection().unwrap()
    }
}

fn outcome(path: &str, disposition: NoteDisposition) -> NoteOutcome {
    NoteOutcome {
        path: path.to_owned(),
        disposition,
    }
}

fn splice(path: &str, base: &str, start: u64, end: u64, text: &str) -> FileChange {
    FileChange::Splice {
        path: path.to_owned(),
        base_hash: hash_bytes(base.as_bytes()),
        splices: vec![Splice {
            start,
            end,
            text: text.to_owned(),
        }],
    }
}

#[test]
fn stale_base_or_dependency_is_a_conflict_before_anything_is_published() {
    let f = Fixture::new();
    let base = "# T\nold\n";
    let topic = f.topic("t", base);
    let evidence = f.topic("evidence", "# E\ncovered\n");
    let note = f.note("n", "note", 1);
    let lease = f.claim(10, 10_000, &[]).unwrap();
    f.topic("t", "# T\nedited meanwhile\n");

    let stale_base = f.store.commit(
        &lease,
        &BatchCommit {
            changes: vec![splice(&topic, base, 4, 7, "new")],
            dependencies: vec![],
            outcomes: vec![outcome(&note, NoteDisposition::Applied)],
            capacity_deferrals_uncounted: false,
        },
        11,
    );
    let stale_dependency = f.store.commit(
        &lease,
        &BatchCommit {
            changes: vec![],
            dependencies: vec![FileDependency {
                path: evidence.clone(),
                content_hash: hash_bytes(b"# E\nolder\n"),
            }],
            outcomes: vec![outcome(&note, NoteDisposition::NoChange)],
            capacity_deferrals_uncounted: false,
        },
        11,
    );

    assert!(matches!(stale_base, Err(BatchDreamError::Conflict(_))));
    assert!(matches!(
        stale_dependency,
        Err(BatchDreamError::Conflict(_))
    ));
    assert_eq!("# T\nedited meanwhile\n", f.read(&topic));
    assert!(f.workspace.join(&note).exists());
}

#[test]
fn interrupted_commit_publishes_nothing() {
    let f = Fixture::new();
    let base = "# T\nold\n";
    let topic = f.topic("t", base);
    let note = f.note("n", "note", 1);
    let lease = f.claim(10, 10_000, &[]).unwrap();
    let interrupted = BatchDreamStore::open(
        &f.workspace,
        V2MemoryScope::Workspace,
        &f.global,
        &f.workspace,
    )
    .unwrap()
    .with_control(BatchDreamControl::with_check_budget(3));

    let result = interrupted.commit(
        &lease,
        &BatchCommit {
            changes: vec![splice(&topic, base, 4, 7, "new")],
            dependencies: vec![],
            outcomes: vec![outcome(&note, NoteDisposition::Applied)],
            capacity_deferrals_uncounted: false,
        },
        11,
    );

    assert!(matches!(result, Err(BatchDreamError::Interrupted)));
    assert_eq!(base, f.read(&topic));
    let status: String = f
        .state()
        .query_row(
            "SELECT status FROM batch_dream_batches WHERE operation_id = ?1",
            params![lease.operation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!("claimed", status);
}

#[test]
fn persisted_plan_is_rolled_forward_after_a_crash() {
    let f = Fixture::new();
    let base = "# T\nold\n";
    let topic = f.topic("t", base);
    let note = f.note("n", "note", 1);
    let lease = f.claim(10, 10_000, &[]).unwrap();
    f.store
        .persist_plan_as(
            &lease,
            &BatchCommit {
                changes: vec![splice(&topic, base, 4, 7, "new")],
                dependencies: vec![],
                outcomes: vec![outcome(&note, NoteDisposition::Applied)],
                capacity_deferrals_uncounted: false,
            },
            11,
            "deferred",
        )
        .unwrap();
    assert_eq!(base, f.read(&topic));

    assert_eq!(0, f.store.recover(12).unwrap());
    assert_eq!(1, f.store.recover(10 + 60).unwrap());

    assert_eq!("# T\nnew\n", f.read(&topic));
    assert!(!f.workspace.join(&note).exists());
    assert_eq!("note", f.read("archive/batch_dream_1/n.md"));
}

#[test]
fn a_note_deferred_too_often_is_archived_as_unplaceable() {
    let f = Fixture::new();
    let note = f.note("orphan", "no home", 1);
    let defer = |reason: &str, capacity_deferrals_uncounted: bool| BatchCommit {
        outcomes: vec![outcome(
            &note,
            NoteDisposition::Deferred {
                reason: reason.to_owned(),
            },
        )],
        capacity_deferrals_uncounted,
        ..BatchCommit::default()
    };
    let now = std::cell::Cell::new(10);
    let run = |commit: &BatchCommit| {
        let lease = f.claim(now.get(), 10_000, &[]).unwrap();
        let report = f.store.commit(&lease, commit, now.get() + 1).unwrap();
        now.set(now.get() + 10);
        (report, lease.operation_id)
    };

    for reason in ["first", "second"] {
        let (report, _) = run(&defer(reason, false));
        assert_eq!(
            (vec![note.clone()], 0),
            (report.deferred, report.unplaceable.len())
        );
    }
    // A release (model failure, cancel) is not a deferral.
    let lease = f.claim(now.get(), 10_000, &[]).unwrap();
    f.store
        .release(&lease, "model failed", now.get() + 1)
        .unwrap();
    now.set(now.get() + 10);
    // A capacity deferral after a cut-off reply is not counted either...
    let (report, _) = run(&defer(CAPACITY_DEFERRAL_REASON, true));
    assert_eq!(vec![note.clone()], report.deferred);
    assert_eq!(Some(2), f.store.queued_deferrals(&note).unwrap());
    // ...but without the runner's flag the same reason counts like any other.
    let (report, operation_id) = run(&defer(CAPACITY_DEFERRAL_REASON, false));

    assert_eq!(vec![note.clone()], report.unplaceable);
    assert!(report.deferred.is_empty());
    assert!(!f.workspace.join(&note).exists());
    assert_eq!(
        "no home",
        f.read(&format!("archive/{operation_id}/orphan.md"))
    );
    assert_eq!(None, f.claim(now.get() + 20, 10_000, &[]));
}

#[test]
fn response_schema_uses_only_portable_keywords() {
    fn walk(value: &serde_json::Value, depth: usize) {
        match value {
            serde_json::Value::Object(map) => {
                if map.get("type").and_then(|kind| kind.as_str()) == Some("object")
                    || map.get("properties").is_some()
                {
                    assert_eq!(
                        map.get("additionalProperties"),
                        Some(&serde_json::Value::Bool(false)),
                        "object node without additionalProperties: false"
                    );
                    let properties: Vec<&str> = map
                        .get("properties")
                        .and_then(|value| value.as_object())
                        .map(|object| object.keys().map(String::as_str).collect())
                        .unwrap_or_default();
                    let required: Vec<&str> = map
                        .get("required")
                        .and_then(|value| value.as_array())
                        .map(|list| list.iter().filter_map(|item| item.as_str()).collect())
                        .unwrap_or_default();
                    for name in &properties {
                        assert!(required.contains(name), "property `{name}` is not required");
                    }
                }
                for (key, child) in map {
                    let banned = matches!(
                        key.as_str(),
                        "const"
                            | "minLength"
                            | "maxLength"
                            | "minimum"
                            | "maximum"
                            | "minItems"
                            | "maxItems"
                            | "pattern"
                    ) || (depth == 0
                        && matches!(key.as_str(), "anyOf" | "oneOf" | "allOf"));
                    assert!(!banned, "schema keyword `{key}` is not portable");
                    walk(child, depth + 1);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, depth + 1)),
            _ => {}
        }
    }
    let schema = response_schema();
    walk(&schema, 0);
    assert_eq!(
        schema.get("required"),
        Some(&serde_json::json!(["actions", "plan"]))
    );
}
