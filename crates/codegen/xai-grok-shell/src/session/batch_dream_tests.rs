use super::*;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::Mutex;
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    global: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new(topics: &[(&str, &str)], notes: &[&str]) -> Fixture {
        let temp = TempDir::new().unwrap();
        let global = temp.path().join("global");
        let workspace = temp.path().join("workspace");
        xai_grok_memory::ensure_scope_initialized(temp.path(), &global, V2MemoryScope::Global)
            .unwrap();
        xai_grok_memory::ensure_scope_initialized(
            temp.path(),
            &workspace,
            V2MemoryScope::Workspace,
        )
        .unwrap();
        for (name, body) in topics {
            std::fs::write(workspace.join(format!("topics/{name}")), body).unwrap();
        }
        let fixture = Fixture {
            _temp: temp,
            global,
            workspace,
        };
        for (index, body) in notes.iter().enumerate() {
            fixture.write_note(index, body);
        }
        fixture
    }

    fn write_note(&self, index: usize, body: &str) {
        let path = self
            .workspace
            .join(format!("observations/_inbox/note-{index}.md"));
        std::fs::write(&path, body).unwrap();
        let modified = filetime::FileTime::from_unix_time(1_000 + index as i64, 0);
        filetime::set_file_mtime(&path, modified).unwrap();
    }

    fn options(&self) -> BatchDreamOptions {
        BatchDreamOptions {
            global_dir: self.global.clone(),
            workspace_dir: self.workspace.clone(),
            owner: "test".to_owned(),
            clock: xai_grok_memory::system_v2_clock(),
            max_run_time: Duration::from_secs(60),
            max_calls_per_batch: 6,
            max_batch_note_bytes: 96 * 1024,
            max_request_bytes: 1 << 20,
        }
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.workspace.join(relative)).unwrap()
    }

    fn inbox(&self) -> usize {
        std::fs::read_dir(self.workspace.join("observations/_inbox"))
            .unwrap()
            .count()
    }
}

#[derive(Clone, Default)]
struct Script {
    replies: Arc<Mutex<VecDeque<Result<ModelReply, BatchDreamStop>>>>,
    requests: Arc<Mutex<Vec<ConversationRequest>>>,
}

impl Script {
    fn new(replies: Vec<Value>) -> Script {
        Script {
            replies: Arc::new(Mutex::new(
                replies
                    .into_iter()
                    .map(|reply| {
                        Ok(ModelReply {
                            text: reply.to_string(),
                            truncated: false,
                        })
                    })
                    .collect(),
            )),
            requests: Arc::default(),
        }
    }

    /// Queue a reply the sampler reports as cut off at the output limit.
    fn truncated(self, text: &str) -> Script {
        self.replies.lock().unwrap().push_back(Ok(ModelReply {
            text: text.to_owned(),
            truncated: true,
        }));
        self
    }

    fn sampler(
        &self,
    ) -> impl FnMut(ConversationRequest) -> std::future::Ready<Result<ModelReply, BatchDreamStop>>
    {
        let script = self.clone();
        move |request| {
            script.requests.lock().unwrap().push(request);
            let reply = script.replies.lock().unwrap().pop_front();
            std::future::ready(reply.unwrap_or(Err(BatchDreamStop::Model)))
        }
    }

    fn request(&self, index: usize) -> ConversationRequest {
        self.requests.lock().unwrap().get(index).cloned().unwrap()
    }
}

fn text(item: &ConversationItem) -> String {
    serde_json::to_value(item).unwrap().to_string()
}

async fn run(fixture: &Fixture, script: &Script) -> BatchDreamReport {
    run_batch_dream(
        fixture.options(),
        tokio_util::sync::CancellationToken::new(),
        script.sampler(),
    )
    .await
}

#[tokio::test]
async fn one_batch_reads_then_commits_in_two_calls() {
    let f = Fixture::new(
        &[(
            "deploys.md",
            "# Deploys\nHow deploys run.\n## Timing\nRollouts take 41 seconds.\n",
        )],
        &["rollouts now take 89 seconds", "deploys exist"],
    );
    let script = Script::new(vec![
        json!({"actions": [{"action": "read_topic", "path": "topics/deploys.md"}]}),
        json!({"plan": {
            "edits": [{"kind": "patch", "id": "E1", "read": "R1", "old_text": "41 seconds", "new_text": "89 seconds"}],
            "outcomes": [
                {"outcome": "applied", "note": "N1", "edits": ["E1"]},
                {"outcome": "no_change", "note": "N2", "evidence": ["R1"]}
            ]
        }}),
    ]);

    let report = run(&f, &script).await;

    assert_eq!(
        BatchDreamReport {
            stop: BatchDreamStop::Drained,
            batches: 1,
            model_calls: 2,
            repairs: 0,
            truncations: 0,
            notes_applied: 1,
            notes_no_change: 1,
            notes_deferred: 0,
            notes_unplaceable: 0,
            topics_changed: 1,
            catalog_topics: 1,
            topic_bytes: 63,
            largest_topic_bytes: 63,
            catalog_tier: Some(CatalogTier::Full),
            limit: None,
        },
        report
    );
    assert_eq!(
        "# Deploys\nHow deploys run.\n## Timing\nRollouts take 89 seconds.\n",
        f.read("topics/deploys.md")
    );
    assert_eq!(0, f.inbox());
    let first = script.request(0);
    assert!(first.tools.is_empty());
    assert!(first.json_schema.is_some());
    assert_eq!(3, first.items.len());
    let catalog = text(first.items.get(1).unwrap());
    assert!(catalog.contains("topics/deploys.md | "), "{catalog}");
    assert!(
        catalog.contains(" | Deploys | How deploys run."),
        "{catalog}"
    );
    assert!(text(first.items.get(2).unwrap()).contains("rollouts now take 89 seconds"));
    assert!(text(script.request(1).items.get(4).unwrap()).contains("Rollouts take 41 seconds."));
}

#[tokio::test]
async fn model_failure_stops_the_run_and_keeps_the_notes() {
    let f = Fixture::new(&[], &["note"]);
    let script = Script::default();

    let report = run(&f, &script).await;

    assert_eq!(BatchDreamStop::Model, report.stop);
    assert_eq!(1, f.inbox());
    let again = Script::new(vec![json!({"plan": {"edits": [], "outcomes": [
        {"outcome": "deferred", "note": "N1", "reason": "later"}
    ]}})]);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let report = run(&f, &again).await;
            if report.stop != BatchDreamStop::Busy {
                assert_eq!(1, report.batches);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_cut_off_reply_is_answered_with_a_shorter_plan_request() {
    let f = Fixture::new(
        &[("deploys.md", "# Deploys\nRollouts take 41 seconds.\n")],
        &["rollouts now take 89 seconds", "unrelated"],
    );
    let script = Script::new(vec![
        json!({"actions": [{"action": "read_topic", "path": "topics/deploys.md"}]}),
    ])
    .truncated(&format!("{{\"plan\":{{\"edits\":[{}", "x".repeat(3 * 1024)));
    script.replies.lock().unwrap().push_back(Ok(ModelReply {
        text: json!({"plan": {
            "edits": [{"kind": "patch", "id": "E1", "read": "R1", "old_text": "41 seconds", "new_text": "89 seconds"}],
            "outcomes": [
                {"outcome": "applied", "note": "N1", "edits": ["E1"]},
                {"outcome": "deferred", "note": "N2", "reason": CAPACITY_DEFERRAL_REASON}
            ]
        }})
        .to_string(),
        truncated: false,
    }));

    let report = run(&f, &script).await;

    assert_eq!(
        (BatchDreamStop::Drained, 3, 0, 1, 1, 1, None),
        (
            report.stop,
            report.model_calls,
            report.repairs,
            report.truncations,
            report.notes_applied,
            report.notes_deferred,
            report.limit
        )
    );
    assert_eq!(
        "# Deploys\nRollouts take 89 seconds.\n",
        f.read("topics/deploys.md")
    );
    let third = script.request(2);
    let head = text(third.items.get(5).unwrap());
    assert!(
        head.contains("[cut off]") && head.len() < 3 * 1024,
        "{}",
        head.len()
    );
    assert!(text(third.items.get(6).unwrap()).contains("cut off at the output limit"));
    // The capacity deferral after a cut-off reply did not count: the note is claimable again with no deferral recorded.
    let store = BatchDreamStore::open(
        &f.workspace,
        V2MemoryScope::Workspace,
        &f.global,
        &f.workspace,
    )
    .unwrap();
    assert_eq!(
        Some(0),
        store
            .queued_deferrals("observations/_inbox/note-1.md")
            .unwrap()
    );
}

#[tokio::test]
async fn repeated_cut_off_replies_release_the_batch() {
    let f = Fixture::new(&[], &["note"]);
    let script = Script::new(vec![])
        .truncated("")
        .truncated("{")
        .truncated("{\"plan\"");

    let report = run(&f, &script).await;

    assert_eq!(
        (
            BatchDreamStop::Drained,
            3,
            MAX_TRUNCATIONS,
            0,
            Some(BatchDreamLimit::Truncations)
        ),
        (
            report.stop,
            report.model_calls,
            report.truncations,
            report.notes_applied,
            report.limit
        )
    );
    assert_eq!(1, f.inbox());
    let first_cut = text(script.request(1).items.get(3).unwrap());
    assert!(first_cut.contains("[cut off]"), "{first_cut}");
}
