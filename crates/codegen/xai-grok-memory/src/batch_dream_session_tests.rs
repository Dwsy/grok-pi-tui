use super::*;
use crate::V2MemoryScope;
use crate::batch_dream::BatchClaimRequest;
use crate::batch_dream_catalog::CATALOG_BUDGET_BYTES;
use crate::batch_dream_plan::BatchPlan;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

const NOW: i64 = 1_000;

struct Fixture {
    _temp: TempDir,
    workspace: PathBuf,
    session: BatchDreamSession,
}

impl Fixture {
    fn new(topics: &[(&str, &str)], notes: &[&str]) -> Fixture {
        let temp = TempDir::new().unwrap();
        let global = temp.path().join("global");
        let workspace = temp.path().join("workspace");
        crate::ensure_scope_initialized(temp.path(), &global, V2MemoryScope::Global).unwrap();
        crate::ensure_scope_initialized(temp.path(), &workspace, V2MemoryScope::Workspace).unwrap();
        for (name, body) in topics {
            std::fs::write(workspace.join(format!("topics/{name}")), body).unwrap();
        }
        for (index, body) in notes.iter().enumerate() {
            let path = workspace.join(format!("observations/_inbox/note-{index}.md"));
            std::fs::write(&path, body).unwrap();
            let modified = filetime::FileTime::from_unix_time(index as i64 + 1, 0);
            filetime::set_file_mtime(&path, modified).unwrap();
        }
        let store = Arc::new(
            BatchDreamStore::open(&workspace, V2MemoryScope::Workspace, &global, &workspace)
                .unwrap(),
        );
        let catalog = Arc::new(TopicCatalog::build(&store, CATALOG_BUDGET_BYTES).unwrap());
        let lease = store
            .claim(&BatchClaimRequest {
                owner: "test",
                now: NOW,
                duration: Duration::from_secs(600),
                max_note_bytes: 64 * 1024,
                excluded: &[],
            })
            .unwrap()
            .unwrap();
        let session = BatchDreamSession::new(
            store,
            catalog,
            lease,
            SessionBudgets {
                read_bytes: 128 * 1024,
            },
        );
        Fixture {
            _temp: temp,
            workspace,
            session,
        }
    }

    fn run(&mut self, actions: Value) -> Vec<Value> {
        let actions: Vec<BatchAction> = serde_json::from_value(actions).unwrap();
        match self.session.dispatch(&actions, NOW + 1).unwrap() {
            Value::Array(results) => results,
            other => panic!("expected results array, got {other}"),
        }
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.workspace.join(relative)).unwrap()
    }

    fn note_path(&self, index: usize) -> String {
        format!("observations/_inbox/note-{index}.md")
    }
}

fn plan(value: Value) -> BatchPlan {
    serde_json::from_value(value).unwrap()
}

#[test]
fn every_edit_kind_resolves_to_exact_bytes() {
    let deploys = "# Deploys\nOld description.\n## Timing\nRollouts take 41 seconds.\n## Alerts\nPage on failure.\n";
    let mut f = Fixture::new(
        &[
            ("deploys.md", deploys),
            ("tips.md", "# Tips\n## Shell\nuse zsh\n"),
        ],
        &[
            "rollouts take 89 seconds",
            "alerts go to #ops",
            "new cache topic",
            "tips unchanged",
        ],
    );
    f.run(json!([
        {"action": "read_topic", "path": "topics/deploys.md"},
        {"action": "read_topic", "path": "topics/tips.md"},
    ]));

    let check = f
        .session
        .check(&plan(json!({
            "edits": [
                {"kind": "patch", "id": "E1", "read": "R1", "old_text": "41 seconds", "new_text": "89 seconds"},
                {"kind": "insert", "id": "E2", "read": "R1", "heading": "## Alerts", "text": "Alerts go to #ops."},
                {"kind": "update_description", "id": "E3", "read": "R1", "description": "How deploys run."},
                {"kind": "replace_section", "id": "E4", "read": "R2", "heading": "## Shell", "text": "## Shell\nuse fish\n"},
                {"kind": "create", "id": "E5", "path": "topics/cache.md", "content": "# Cache\nBuild cache notes."}
            ],
            "outcomes": [
                {"outcome": "applied", "note": "N1", "edits": ["E1", "E3"]},
                {"outcome": "applied", "note": "N2", "edits": ["E2"]},
                {"outcome": "applied", "note": "N3", "edits": ["E4", "E5"]},
                {"outcome": "no_change", "note": "N4", "evidence": ["R2"]}
            ]
        })))
        .unwrap();

    assert_eq!(Vec::<String>::new(), check.errors);
    f.session.commit(&check.commit, NOW + 2).unwrap();
    assert_eq!(
        "# Deploys\nHow deploys run.\n## Timing\nRollouts take 89 seconds.\n## Alerts\nPage on failure.\nAlerts go to #ops.\n",
        f.read("topics/deploys.md")
    );
    assert_eq!("# Tips\n## Shell\nuse fish\n", f.read("topics/tips.md"));
    assert_eq!("# Cache\nBuild cache notes.\n", f.read("topics/cache.md"));
    assert!(!f.workspace.join(f.note_path(3)).exists());
}
