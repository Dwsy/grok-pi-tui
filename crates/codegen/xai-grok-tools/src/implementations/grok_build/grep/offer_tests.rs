use std::path::Path;
use std::sync::Arc;

use futures::{FutureExt, StreamExt};
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;

use super::*;
use crate::computer::content_search::{
    ContentSearch, ContentSearchFailed, FileHits, HitKind, HitLine,
};
use crate::computer::local::MockFs;
use crate::computer::types::{AsyncFileSystem, ComputerError};
use crate::implementations::grok_build::grep::card::grep_timeout_output;
use crate::implementations::grok_build::grep::tests::{make_grep_input, read_grep_delta};
use crate::implementations::grok_build::grep::{GrepSearchInput, GrepTool};
use crate::types::resources::{Cwd, DenyReadGlobs, FileSystem, Resources, SharedResources};
use crate::types::tool_metadata::test_ctx;

/// What the fake backend's job answers.
#[derive(Clone)]
enum Answer {
    /// Hits by path below the request root.
    Hits(ContentSearchOutcome, Vec<(&'static str, Vec<HitLine>)>),
    Fail(&'static str),
    /// Never completes, so the tool deadline decides.
    Pending,
}

/// Offers every search, answers with `answer`, and reports each request.
struct OfferingFs {
    inner: MockFs,
    answer: Answer,
    requests: mpsc::UnboundedSender<ContentSearchRequest>,
}

#[async_trait::async_trait]
impl AsyncFileSystem for OfferingFs {
    async fn read_file(&self, path: &Path) -> Result<Vec<u8>, ComputerError> {
        self.inner.read_file(path).await
    }

    async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), ComputerError> {
        self.inner.write_file(path, data).await
    }

    async fn delete_file(&self, path: &Path) -> Result<(), ComputerError> {
        self.inner.delete_file(path).await
    }

    fn offer_content_search(&self, request: &ContentSearchRequest) -> Option<ContentSearchJob> {
        self.requests.send(request.clone()).unwrap();
        let answer = self.answer.clone();
        let root = request.root.clone();
        Some(
            async move {
                match answer {
                    Answer::Hits(outcome, files) => Ok(ContentSearch {
                        files: files
                            .into_iter()
                            .map(|(path, lines)| FileHits {
                                path: root.join(path),
                                lines,
                            })
                            .collect(),
                        outcome,
                    }),
                    Answer::Fail(label) => Err(ContentSearchFailed { label }),
                    Answer::Pending => futures::future::pending().await,
                }
            }
            .boxed(),
        )
    }
}

fn offering_fs(
    answer: Answer,
) -> (
    Arc<dyn AsyncFileSystem>,
    mpsc::UnboundedReceiver<ContentSearchRequest>,
) {
    let (requests, received) = mpsc::unbounded_channel();
    let fs = OfferingFs {
        inner: MockFs::new(),
        answer,
        requests,
    };
    (Arc::new(fs), received)
}

fn matched(line_no: u64, text: &str) -> HitLine {
    HitLine {
        line_no,
        kind: HitKind::Match,
        text: text.as_bytes().to_vec(),
        is_cut: false,
    }
}

/// `a.txt` with two matches and `five/five.txt` with five. Searches that reach
/// `rg` stay inside one file, since `rg` orders files nondeterministically.
fn tree() -> TempDir {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "needle one\nplain\nneedle two\n").unwrap();
    std::fs::create_dir(tmp.path().join("five")).unwrap();
    let five: String = (1..=5).map(|i| format!("needle {i}\n")).collect();
    std::fs::write(tmp.path().join("five/five.txt"), five).unwrap();
    tmp
}

fn five_hits() -> Vec<(&'static str, Vec<HitLine>)> {
    let lines = (1..=5)
        .map(|i| matched(i, &format!("needle {i}")))
        .collect();
    vec![("five.txt", lines)]
}

fn resources(root: &Path, file_system: Option<&Arc<dyn AsyncFileSystem>>) -> SharedResources {
    denying_resources(root, file_system, &[])
}

fn denying_resources(
    root: &Path,
    file_system: Option<&Arc<dyn AsyncFileSystem>>,
    deny_read_globs: &[&str],
) -> SharedResources {
    let mut resources = Resources::new();
    resources.insert(Cwd(root.to_path_buf()));
    if let Some(file_system) = file_system {
        resources.insert(FileSystem(Arc::clone(file_system)));
    }
    if !deny_read_globs.is_empty() {
        resources.insert(DenyReadGlobs(
            deny_read_globs
                .iter()
                .map(|&glob| glob.to_owned())
                .collect(),
        ));
    }
    resources.into_shared()
}

/// A grep result with readable bytes, comparable as a whole.
#[derive(Debug, PartialEq)]
struct Card {
    stdout: String,
    stderr: String,
    exit_code: i32,
    match_count: usize,
    file_matches: serde_json::Value,
}

impl From<GrepSearchOutput> for Card {
    fn from(output: GrepSearchOutput) -> Card {
        Card {
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
            exit_code: output.exit_code,
            match_count: output.match_count,
            file_matches: serde_json::to_value(output.file_matches).unwrap(),
        }
    }
}

/// The card body: the lines between the "Found …" summary and the closing tag,
/// empty for a card without a summary.
fn card_body(card: &Card) -> &str {
    if !card.stdout.contains("\nFound ") {
        return "";
    }
    let mut parts = card.stdout.splitn(3, '\n');
    let (Some(_open), Some(_summary), Some(rest)) = (parts.next(), parts.next(), parts.next())
    else {
        panic!("card without a body: {card:?}");
    };
    rest.split_once("\n</workspace_result>").unwrap().0
}

/// Runs the blocking and the streaming path, checks they agree and that the
/// streamed deltas are the card body, and returns the card.
async fn run_both(resources: &SharedResources, input: GrepSearchInput) -> Card {
    let blocking = Card::from(
        xai_tool_runtime::Tool::run(&GrepTool, test_ctx(Arc::clone(resources)), input.clone())
            .await
            .unwrap(),
    );
    let mut stream =
        xai_tool_runtime::Tool::execute(&GrepTool, test_ctx(Arc::clone(resources)), input).await;
    let mut deltas = String::new();
    let mut terminal = None;
    while let Some(item) = stream.next().await {
        match item {
            xai_tool_runtime::ToolStreamItem::Progress(p) => deltas.push_str(&read_grep_delta(&p)),
            xai_tool_runtime::ToolStreamItem::Terminal(result) => {
                terminal = Some(Card::from(result.unwrap()));
            }
        }
    }
    let streamed = terminal.unwrap();
    assert_eq!(blocking, streamed, "blocking and streamed cards differ");
    assert_eq!(card_body(&blocking), deltas, "deltas are not the card body");
    blocking
}

fn offers(received: &mut mpsc::UnboundedReceiver<ContentSearchRequest>) -> usize {
    std::iter::from_fn(|| received.try_recv().ok()).count()
}

#[tokio::test]
async fn served_content_is_the_card_rg_gives() {
    let tree = tree();
    let answer = Answer::Hits(
        ContentSearchOutcome::Complete,
        vec![(
            "a.txt",
            vec![matched(1, "needle one"), matched(3, "needle two")],
        )],
    );
    let (fs, mut received) = offering_fs(answer);
    let mut input = make_grep_input("needle");
    input.glob = Some("a.txt".to_owned());

    let served = run_both(&resources(tree.path(), Some(&fs)), input.clone()).await;
    let from_rg = run_both(&resources(tree.path(), None), input).await;

    assert_eq!(from_rg, served);
    assert_eq!(2, offers(&mut received));
}

#[tokio::test]
async fn served_no_match_is_the_no_match_card() {
    let tree = tree();
    let (fs, mut received) = offering_fs(Answer::Hits(ContentSearchOutcome::Complete, Vec::new()));
    let input = make_grep_input("absent_pattern_xyz");

    let served = run_both(&resources(tree.path(), Some(&fs)), input.clone()).await;
    let from_rg = run_both(&resources(tree.path(), None), input).await;

    assert_eq!(from_rg, served);
    assert_eq!(1, served.exit_code);
    assert_eq!(2, offers(&mut received));
}

#[tokio::test]
async fn served_truncated_result_past_the_head_limit_says_at_least() {
    let tree = tree();
    let (fs, _received) = offering_fs(Answer::Hits(ContentSearchOutcome::Truncated, five_hits()));
    let mut input = make_grep_input("needle");
    input.path = Some("five".to_owned());
    input.head_limit = Some(3);

    let served = run_both(&resources(tree.path(), Some(&fs)), input.clone()).await;
    let from_rg = run_both(&resources(tree.path(), None), input).await;

    assert_eq!(from_rg, served);
    assert!(
        served.stdout.contains("Found at least 2 matching lines"),
        "{served:?}"
    );
}

#[tokio::test]
async fn served_exact_fit_is_not_truncated() {
    let tree = tree();
    let (fs, _received) = offering_fs(Answer::Hits(ContentSearchOutcome::Complete, five_hits()));
    let mut input = make_grep_input("needle");
    input.path = Some("five".to_owned());
    input.head_limit = Some(6);

    let served = run_both(&resources(tree.path(), Some(&fs)), input.clone()).await;
    let from_rg = run_both(&resources(tree.path(), None), input).await;

    assert_eq!(from_rg, served);
    assert!(
        served.stdout.contains("Found 5 matching lines"),
        "{served:?}"
    );
}

/// A `Truncated` result that fits the card would read as complete, so `rg`
/// answers instead (here with both matches, not the one served hit).
#[tokio::test]
async fn truncated_result_the_card_cannot_show_runs_rg() {
    let tree = tree();
    let answer = Answer::Hits(
        ContentSearchOutcome::Truncated,
        vec![("a.txt", vec![matched(1, "needle one")])],
    );
    let (fs, mut received) = offering_fs(answer);
    let mut input = make_grep_input("needle");
    input.glob = Some("a.txt".to_owned());

    let answered = run_both(&resources(tree.path(), Some(&fs)), input.clone()).await;
    let from_rg = run_both(&resources(tree.path(), None), input).await;

    assert_eq!(from_rg, answered);
    assert_eq!(2, answered.match_count);
    assert_eq!(2, offers(&mut received));
}

#[tokio::test]
async fn failed_job_runs_rg() {
    let tree = tree();
    let (fs, mut received) = offering_fs(Answer::Fail("test_failure"));
    let mut input = make_grep_input("needle");
    input.path = Some("five".to_owned());

    let answered = run_both(&resources(tree.path(), Some(&fs)), input.clone()).await;
    let from_rg = run_both(&resources(tree.path(), None), input).await;

    assert_eq!(from_rg, answered);
    assert_eq!(5, answered.match_count);
    assert_eq!(2, offers(&mut received));
}

#[tokio::test]
async fn declining_file_system_leaves_rg_output_unchanged() {
    let tree = tree();
    let declining: Arc<dyn AsyncFileSystem> = Arc::new(MockFs::new());
    let mut input = make_grep_input("needle");
    input.path = Some("five".to_owned());

    let answered = run_both(&resources(tree.path(), Some(&declining)), input.clone()).await;
    let from_rg = run_both(&resources(tree.path(), None), input).await;

    assert_eq!(from_rg, answered);
}

#[tokio::test]
async fn timed_out_job_with_hits_returns_the_partial_card() {
    let tree = tree();
    let answer = Answer::Hits(
        ContentSearchOutcome::TimedOut,
        vec![("a.txt", vec![matched(1, "needle one")])],
    );
    let (fs, _received) = offering_fs(answer);
    let root = tree.path().display();
    let secs = grep_timeout().as_secs();

    let card = run_both(
        &resources(tree.path(), Some(&fs)),
        make_grep_input("needle"),
    )
    .await;

    let expected = Card {
        stdout: format!(
            "<workspace_result workspace_path=\"{root}\">\nFound at least 1 matching lines\n\
             {root}/a.txt\n1:needle one\n</workspace_result>\n\
             Ripgrep search timed out after {secs} seconds; the matches above are partial. \
             Try searching a more specific path or pattern."
        ),
        stderr: String::new(),
        exit_code: -1,
        match_count: 1,
        file_matches: serde_json::json!([{
            "path": format!("{root}/a.txt"),
            "matches": [{"line_number": 1, "content": "needle one"}],
        }]),
    };
    assert_eq!(expected, card);
}

#[tokio::test]
async fn timed_out_job_without_hits_returns_the_timeout_card() {
    let tree = tree();
    let (fs, _received) = offering_fs(Answer::Hits(ContentSearchOutcome::TimedOut, Vec::new()));

    let card = run_both(
        &resources(tree.path(), Some(&fs)),
        make_grep_input("needle"),
    )
    .await;

    assert_eq!(
        Card::from(grep_timeout_output(grep_timeout().as_secs())),
        card
    );
}

#[tokio::test(start_paused = true)]
async fn job_still_running_at_the_deadline_returns_the_timeout_card() {
    let tree = tree();
    let (fs, mut received) = offering_fs(Answer::Pending);

    let card = run_both(
        &resources(tree.path(), Some(&fs)),
        make_grep_input("needle"),
    )
    .await;

    assert_eq!(
        Card::from(grep_timeout_output(grep_timeout().as_secs())),
        card
    );
    assert_eq!(2, offers(&mut received));
}

#[tokio::test]
async fn request_describes_the_rg_call() {
    let tree = tree();
    let (fs, mut received) = offering_fs(Answer::Hits(ContentSearchOutcome::Complete, Vec::new()));
    let mut resources = Resources::new();
    resources.insert(Cwd(tree.path().to_path_buf()));
    resources.insert(FileSystem(Arc::clone(&fs)));
    resources.insert(DenyReadGlobs(vec!["**/*.pem".to_owned()]));
    let resources = resources.into_shared();
    let input = GrepSearchInput {
        path: Some("five".to_owned()),
        glob: Some("*.txt".to_owned()),
        output_mode: Some(OutputMode::FilesWithMatches),
        context: Some(2),
        before_context: Some(1),
        after_context: Some(0),
        case_insensitive: true,
        r#type: Some("txt".to_owned()),
        head_limit: Some(7),
        multiline: true,
        ..make_grep_input("needle")
    };

    let before = tokio::time::Instant::now();
    xai_tool_runtime::Tool::run(&GrepTool, test_ctx(Arc::clone(&resources)), input)
        .await
        .unwrap();
    let after = tokio::time::Instant::now();
    let file_input = GrepSearchInput {
        path: Some("a.txt".to_owned()),
        ..make_grep_input("needle")
    };
    xai_tool_runtime::Tool::run(&GrepTool, test_ctx(resources), file_input)
        .await
        .unwrap();

    let request = received.try_recv().unwrap();
    let expected = ContentSearchRequest {
        root: tree.path().join("five"),
        root_kind: RootKind::Directory,
        pattern: "needle".to_owned(),
        case: CaseSensitivity::Insensitive,
        globs: vec!["*.txt".to_owned(), "!**/*.pem".to_owned()],
        file_type: Some("txt".to_owned()),
        multiline: MultilineMode::DotAll,
        context_before: 1,
        context_after: 2,
        mode: ContentSearchMode::FilesWithMatches,
        max_file_bytes: 5 * 1024 * 1024,
        max_columns: 1000,
        result_budget: 8,
        process_cwd: std::env::current_dir().unwrap(),
        deadline: request.deadline,
    };
    assert_eq!(expected, request);
    assert!(
        before + grep_timeout() <= request.deadline && request.deadline <= after + grep_timeout()
    );
    let file_request = received.try_recv().unwrap();
    assert_eq!(
        (tree.path().join("a.txt"), RootKind::File),
        (file_request.root, file_request.root_kind)
    );
}

/// A served hit in a file a read-deny glob excludes is dropped, and `rg`, which
/// never reads that file, answers instead.
#[tokio::test]
async fn served_hit_in_a_denied_file_runs_rg() {
    let tree = tree();
    std::fs::write(tree.path().join("five/secret.pem"), "needle secret\n").unwrap();
    let mut hits = five_hits();
    hits.push(("secret.pem", vec![matched(1, "needle secret")]));
    let (fs, mut received) = offering_fs(Answer::Hits(ContentSearchOutcome::Complete, hits));
    let mut input = make_grep_input("needle");
    input.path = Some("five".to_owned());

    let deny = ["**/*.pem"];
    let answered = run_both(
        &denying_resources(tree.path(), Some(&fs), &deny),
        input.clone(),
    )
    .await;
    let from_rg = run_both(&denying_resources(tree.path(), None, &deny), input).await;

    assert_eq!(from_rg, answered);
    assert!(!answered.stdout.contains("secret"), "{answered:?}");
    assert_eq!(2, offers(&mut received));
}

/// A glob that excludes a directory excludes every file below it, as in `rg`'s
/// walk.
#[tokio::test]
async fn served_hit_below_a_denied_directory_runs_rg() {
    let tree = tree();
    std::fs::create_dir(tree.path().join("five/secrets")).unwrap();
    std::fs::write(tree.path().join("five/secrets/key.txt"), "needle key\n").unwrap();
    let mut hits = five_hits();
    hits.push(("secrets/key.txt", vec![matched(1, "needle key")]));
    let (fs, _received) = offering_fs(Answer::Hits(ContentSearchOutcome::Complete, hits));
    let mut input = make_grep_input("needle");
    input.path = Some("five".to_owned());

    let deny = ["**/secrets"];
    let answered = run_both(
        &denying_resources(tree.path(), Some(&fs), &deny),
        input.clone(),
    )
    .await;
    let from_rg = run_both(&denying_resources(tree.path(), None, &deny), input).await;

    assert_eq!(from_rg, answered);
    assert!(!answered.stdout.contains("needle key"), "{answered:?}");
}

#[tokio::test]
async fn served_hit_outside_the_root_runs_rg() {
    let tree = tree();
    let answer = Answer::Hits(
        ContentSearchOutcome::Complete,
        vec![("../a.txt", vec![matched(1, "needle one")])],
    );
    let (fs, _received) = offering_fs(answer);
    let mut input = make_grep_input("needle");
    input.path = Some("five".to_owned());

    let answered = run_both(&resources(tree.path(), Some(&fs)), input.clone()).await;
    let from_rg = run_both(&resources(tree.path(), None), input).await;

    assert_eq!(from_rg, answered);
    assert_eq!(5, answered.match_count);
}

/// `rg -c -U` counts multiline matches, which the hits do not delimit, so the
/// tool does not offer a multiline count.
#[tokio::test]
async fn multiline_count_is_not_offered() {
    let tree = tree();
    let (fs, mut received) = offering_fs(Answer::Fail("unreachable"));
    let input = GrepSearchInput {
        path: Some("five".to_owned()),
        output_mode: Some(OutputMode::Count),
        multiline: true,
        ..make_grep_input("needle")
    };

    let answered = run_both(&resources(tree.path(), Some(&fs)), input.clone()).await;
    let from_rg = run_both(&resources(tree.path(), None), input).await;

    assert_eq!(from_rg, answered);
    assert_eq!(0, offers(&mut received));
}

/// A job that fails leaves `rg` only what remains of the request deadline, so a
/// served-then-failed call stays within one grep timeout.
#[tokio::test]
async fn failed_job_leaves_rg_the_rest_of_the_deadline() {
    let tree = tree();
    let input = make_grep_input("needle");
    let output_mode = OutputMode::Content;
    let args = RgArgs {
        program: crate::implementations::grok_build::grep::ripgrep::rg_path().unwrap(),
        input: &input,
        output_mode: &output_mode,
        root: tree.path(),
        globs: &[],
    };
    let mut request = content_search_request(&args, RootKind::Directory, 10).unwrap();
    request.deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    let source = GrepSource::Offered {
        job: async {
            Err(ContentSearchFailed {
                label: "test_failure",
            })
        }
        .boxed(),
        request: request.clone(),
        rg_command: args.into_command(),
    };
    let config = GrepFormatConfig {
        output_mode,
        effective_head_limit: 10,
        max_chars_per_line: 1000,
        max_output_bytes: 100_000,
        cwd_display: tree.path().display().to_string(),
    };

    let resolved = source.resolve(&config, &tracing::Span::none()).await;

    let ResolvedSource::Rg { mut rg, deadline } = resolved else {
        panic!("a failed job falls back to rg");
    };
    rg.kill_and_reap().await;
    assert_eq!(request.deadline, deadline);
}

#[test]
fn rg_reads_user_config_only_when_the_path_is_set_and_not_empty() {
    assert_eq!(
        [false, false, true],
        [
            None,
            Some(OsStr::new("")),
            Some(OsStr::new("/home/u/.ripgreprc"))
        ]
        .map(rg_reads_user_config)
    );
}
