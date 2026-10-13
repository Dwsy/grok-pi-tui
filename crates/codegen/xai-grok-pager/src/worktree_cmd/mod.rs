mod display;
use crate::app::worktree_session::{WorktreeSpec, create_worktree, new_worktree_id};
use agent_client_protocol as acp;
use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::io::Write;
use std::path::Path;
use tokio_util::sync::CancellationToken;
use xai_acp_lib::acp_send;
use xai_fast_worktree::WorktreeRecord;
/// Reuse the agent's own report types rather than copies, so a field added there cannot go missing here.
pub use xai_fast_worktree::{DbStats, GcReport, KeptWorktree, RebuildReport};
use xai_grok_shell::agent::config::Config as AgentConfig;
#[derive(Debug, clap::Args, Clone)]
pub struct WorktreeArgs {
    #[command(subcommand)]
    command: WorktreeCommand,
}
#[derive(Debug, Subcommand, Clone)]
enum WorktreeCommand {
    /// Create a worktree the way `grok -w` does, without starting a session
    Create {
        /// Worktree name; generated when omitted
        name: Option<String>,
        /// Branch, tag, or commit to base the worktree on; without it, HEAD plus uncommitted changes
        #[arg(long = "ref", value_name = "REF")]
        git_ref: Option<String>,
    },
    /// List tracked worktrees
    #[command(visible_alias = "ls")]
    List {
        #[arg(long)]
        repo: Option<String>,
        #[arg(long, value_delimiter = ',')]
        r#type: Vec<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        all: bool,
    },
    /// Show details for a specific worktree
    Show { id_or_path: String },
    /// Remove worktrees
    Rm {
        #[arg(required = true)]
        ids: Vec<String>,
        #[arg(short, long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove expired worktrees, keeping any whose work would not survive.
    #[command(alias = "prune")]
    Gc {
        /// Report what would be removed without removing it.
        #[arg(long)]
        dry_run: bool,
        /// Expire worktrees idle longer than this, e.g. `7d`.
        /// Without it, nothing expires.
        #[arg(long)]
        max_age: Option<String>,
        /// Skip the live-process and protected-path guards.
        /// This does not override the safety check; use `grok worktree rm` for that.
        #[arg(short, long)]
        force: bool,
    },
    /// Database maintenance
    Db {
        #[command(subcommand)]
        command: WorktreeDbCommand,
    },
}
#[derive(Debug, Subcommand, Clone)]
enum WorktreeDbCommand {
    /// Rebuild DB from filesystem scan
    Rebuild,
    /// Show DB statistics
    Stats,
    /// Print DB file path
    Path,
}
pub async fn run(args: WorktreeArgs, agent_config: &AgentConfig) -> Result<()> {
    let command = args.command;
    let cancel = CancellationToken::new();
    xai_grok_telemetry::startup::mark_utility_process();
    let spawned = crate::acp::spawn::spawn_grok_shell(agent_config.clone(), &cancel, None).await?;
    let _agent_guard =
        crate::acp::spawn::AgentShutdownGuard::new(cancel.clone(), Some(spawned.thread_handle));
    let _init: acp::InitializeResponse = acp_send(
        acp::InitializeRequest::new(acp::ProtocolVersion::V1)
            .client_capabilities(
                acp::ClientCapabilities::new()
                    .fs(acp::FileSystemCapabilities::new())
                    .terminal(false),
            )
            .meta(
                serde_json::json!({
                    "clientType": crate::client_identity::HEADLESS_CLIENT_TYPE,
                    "clientVersion": crate::client_identity::PAGER_CLIENT_VERSION
                })
                .as_object()
                .cloned(),
            ),
        &spawned.channel.tx,
    )
    .await?;
    dispatch(command, &spawned.channel.tx).await
}
async fn dispatch(command: WorktreeCommand, tx: &xai_acp_lib::AcpAgentTx) -> Result<()> {
    match command {
        WorktreeCommand::Create { name, git_ref } => {
            let source_cwd =
                std::env::current_dir().context("couldn't read the current directory")?;
            let spec = WorktreeSpec {
                label: name,
                git_ref,
            };
            cmd_create(tx, &source_cwd, &spec, &mut std::io::stdout()).await
        }
        WorktreeCommand::List {
            repo,
            r#type,
            json,
            all,
        } => cmd_list(tx, repo, r#type, json, all).await,
        WorktreeCommand::Show { id_or_path } => cmd_show(tx, &id_or_path).await,
        WorktreeCommand::Rm {
            ids,
            force,
            dry_run,
        } => cmd_rm(tx, ids, force, dry_run).await,
        WorktreeCommand::Gc {
            dry_run,
            max_age,
            force,
        } => cmd_gc(tx, dry_run, max_age, force).await,
        WorktreeCommand::Db { command } => cmd_db(tx, command).await,
    }
}
fn ext_request<T: serde::Serialize>(
    method: &str,
    params: &T,
) -> Result<acp::ExtRequest, serde_json::Error> {
    let params = serde_json::value::to_raw_value(params)?;
    Ok(acp::ExtRequest::new(method, params.into()))
}
/// ACP extension responses are wrapped in `{ "result": T, "error": ... }`.
#[derive(serde::Deserialize)]
struct ExtEnvelope<T> {
    result: Option<T>,
    error: Option<serde_json::Value>,
}
async fn ext_call<T: serde::de::DeserializeOwned>(
    tx: &xai_acp_lib::AcpAgentTx,
    method: &str,
    params: &impl serde::Serialize,
) -> Result<T> {
    let req =
        ext_request(method, params).map_err(|e| anyhow::anyhow!("failed to build request: {e}"))?;
    let resp = acp_send(req, tx)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let envelope: ExtEnvelope<T> = serde_json::from_str(resp.0.get())
        .map_err(|e| anyhow::anyhow!("response parse error: {e}"))?;
    if let Some(err) = envelope.error {
        bail!("ACP error: {err}");
    }
    envelope
        .result
        .ok_or_else(|| anyhow::anyhow!("ACP response missing result field"))
}
/// Stdout carries only the session directory, so `cd "$(grok worktree create)"` works.
async fn cmd_create(
    tx: &xai_acp_lib::AcpAgentTx,
    source_cwd: &Path,
    spec: &WorktreeSpec,
    out: &mut impl Write,
) -> Result<()> {
    let created = create_worktree(tx, source_cwd, spec, &new_worktree_id(None)).await?;
    if let Some(summary) = &created.strategy_summary {
        crate::best_effort_stderr::eprint_line(summary);
    }
    let dir = if created.session_cwd.is_dir() {
        &created.session_cwd
    } else {
        crate::best_effort_stderr::eprint_line(&format!(
            "{} is not in the new worktree; printing the worktree root",
            created.session_cwd.display()
        ));
        &created.worktree_root
    };
    let written = writeln!(out, "{}", dir.display());
    Ok(crate::util::ignore_broken_pipe(written)?)
}
async fn cmd_list(
    tx: &xai_acp_lib::AcpAgentTx,
    repo: Option<String>,
    types: Vec<String>,
    json: bool,
    all: bool,
) -> Result<()> {
    let records: Vec<WorktreeRecord> = ext_call(
        tx,
        "x.ai/git/worktree/list",
        &serde_json::json!({
            "repo": repo,
            "type": types,
            "includeAll": all,
        }),
    )
    .await?;
    let mut out = std::io::stdout().lock();
    let written = if json {
        display::print_json(&records, &mut out)
    } else {
        display::print_table(&records, &mut out)
    };
    Ok(crate::util::ignore_broken_pipe(written)?)
}
async fn cmd_show(tx: &xai_acp_lib::AcpAgentTx, id_or_path: &str) -> Result<()> {
    let result: Result<Option<WorktreeRecord>> = ext_call(
        tx,
        "x.ai/git/worktree/show",
        &serde_json::json!({ "idOrPath" : id_or_path }),
    )
    .await;
    let rec = result?;
    match rec {
        Some(r) => {
            let redirections_bytes = None;
            let written =
                display::print_show(&r, redirections_bytes, &mut std::io::stdout().lock());
            Ok(crate::util::ignore_broken_pipe(written)?)
        }
        None => bail!("worktree not found: {id_or_path}"),
    }
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoveResponse {
    removed: bool,
    #[serde(default)]
    resolved_path: Option<String>,
}
async fn cmd_rm(
    tx: &xai_acp_lib::AcpAgentTx,
    ids: Vec<String>,
    force: bool,
    dry_run: bool,
) -> Result<()> {
    for id_or_path in &ids {
        let resp: Result<RemoveResponse> = ext_call(
            tx,
            "x.ai/git/worktree/remove",
            &serde_json::json!({
                "idOrPath": id_or_path,
                "force": force,
                "dryRun": dry_run,
            }),
        )
        .await;
        match resp {
            Ok(r) => {
                let path = r.resolved_path.as_deref().unwrap_or(id_or_path);
                if dry_run {
                    println!("  would remove: {path}");
                } else if r.removed {
                    println!("  removed: {path}");
                }
            }
            Err(e) => eprintln!("  error removing {id_or_path}: {e}"),
        }
    }
    Ok(())
}
async fn cmd_gc(
    tx: &xai_acp_lib::AcpAgentTx,
    dry_run: bool,
    max_age: Option<String>,
    force: bool,
) -> Result<()> {
    let report: GcReport = ext_call(
        tx,
        "x.ai/git/worktree/gc",
        &serde_json::json!({
            "dryRun": dry_run,
            "maxAge": max_age,
            "force": force,
        }),
    )
    .await?;
    let mut out = std::io::stdout().lock();
    let written = (|| {
        if dry_run {
            writeln!(out, "Dry run: no changes made.")?;
        }
        display::print_gc(&report, &mut out)
    })();
    Ok(crate::util::ignore_broken_pipe(written)?)
}
async fn cmd_db(tx: &xai_acp_lib::AcpAgentTx, command: WorktreeDbCommand) -> Result<()> {
    match command {
        WorktreeDbCommand::Stats => {
            let stats: DbStats = ext_call(tx, "x.ai/git/worktree/db/stats", &()).await?;
            let written = display::print_stats(&stats, &mut std::io::stdout().lock());
            Ok(crate::util::ignore_broken_pipe(written)?)
        }
        WorktreeDbCommand::Path => {
            #[derive(serde::Deserialize)]
            struct PathResp {
                path: String,
            }
            let resp: PathResp = ext_call(tx, "x.ai/git/worktree/db/path", &()).await?;
            println!("{}", resp.path);
            Ok(())
        }
        WorktreeDbCommand::Rebuild => {
            let report: RebuildReport = ext_call(tx, "x.ai/git/worktree/db/rebuild", &()).await?;
            let written = display::print_rebuild(&report, &mut std::io::stdout().lock());
            Ok(crate::util::ignore_broken_pipe(written)?)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::worktree_session::{CREATE_METHOD, create_worktree_params};
    /// Answers every extension call with `reply` and forwards each `(method, params)` it saw.
    fn spawn_ext_agent(
        reply: serde_json::Value,
    ) -> (
        xai_acp_lib::AcpAgentTx,
        tokio::sync::mpsc::UnboundedReceiver<(String, serde_json::Value)>,
    ) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpAgentMessage>();
        let (seen_tx, seen_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let xai_acp_lib::AcpAgentMessage::ExtMethod(args) = msg else {
                    panic!("unexpected ACP message");
                };
                let params: serde_json::Value =
                    serde_json::from_str(args.request.params.get()).unwrap();
                seen_tx
                    .send((args.request.method.to_string(), params))
                    .unwrap();
                let raw = serde_json::value::to_raw_value(&reply).unwrap();
                args.response_tx
                    .send(Ok(acp::ExtResponse::new(std::sync::Arc::from(raw))))
                    .unwrap();
            }
        });
        (tx, seen_rx)
    }
    #[tokio::test]
    async fn create_sends_the_dash_w_request_and_prints_only_the_session_cwd() {
        let src = tempfile::tempdir().unwrap();
        let launch_cwd = src.path().join("crates").join("pager");
        std::fs::create_dir_all(&launch_cwd).unwrap();
        let wt = tempfile::tempdir().unwrap();
        let session_cwd = wt.path().join("crates").join("pager");
        std::fs::create_dir_all(&session_cwd).unwrap();
        let (tx, mut seen) = spawn_ext_agent(serde_json::json!({"result": {
            "worktreePath": wt.path(),
            "sourceGitRoot": src.path(),
        }}));
        let spec = WorktreeSpec {
            label: Some("my-fix".to_owned()),
            git_ref: Some("origin/main".to_owned()),
        };
        let mut out = Vec::new();
        cmd_create(&tx, &launch_cwd, &spec, &mut out).await.unwrap();
        let (method, params) = seen.recv().await.unwrap();
        assert_eq!(CREATE_METHOD, method);
        let worktree_id = params.get("newSessionId").and_then(|v| v.as_str()).unwrap();
        assert!(worktree_id.starts_with("pager-"), "{worktree_id}");
        assert_eq!(
            create_worktree_params(&launch_cwd, &spec, worktree_id),
            params
        );
        assert_eq!(
            format!("{}\n", session_cwd.display()),
            String::from_utf8(out).unwrap()
        );
    }
    #[tokio::test]
    async fn create_prints_the_worktree_root_when_the_ref_lacks_the_launch_subdirectory() {
        let src = tempfile::tempdir().unwrap();
        let launch_cwd = src.path().join("crates").join("pager");
        std::fs::create_dir_all(&launch_cwd).unwrap();
        let wt = tempfile::tempdir().unwrap();
        let (tx, _seen) = spawn_ext_agent(serde_json::json!({"result": {
            "worktreePath": wt.path(),
            "sourceGitRoot": src.path(),
        }}));
        let spec = WorktreeSpec {
            label: None,
            git_ref: Some("v1".to_owned()),
        };
        let mut out = Vec::new();
        cmd_create(&tx, &launch_cwd, &spec, &mut out).await.unwrap();
        assert_eq!(
            format!("{}\n", wt.path().display()),
            String::from_utf8(out).unwrap()
        );
    }
    #[tokio::test]
    async fn create_failure_surfaces_the_agent_error_and_prints_nothing() {
        let src = tempfile::tempdir().unwrap();
        let (tx, _seen) = spawn_ext_agent(serde_json::json!({"error": "disk full"}));
        let mut out = Vec::new();
        let error = cmd_create(&tx, src.path(), &WorktreeSpec::default(), &mut out)
            .await
            .unwrap_err();
        assert_eq!("couldn't create worktree: disk full", error.to_string());
        assert!(out.is_empty());
    }
    #[test]
    fn create_parses_name_and_ref_and_defaults_both_to_none() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            command: WorktreeCommand,
        }
        let cli = Cli::parse_from(["test", "create", "my-fix", "--ref", "origin/main"]);
        match cli.command {
            WorktreeCommand::Create { name, git_ref } => {
                assert_eq!(Some("my-fix"), name.as_deref());
                assert_eq!(Some("origin/main"), git_ref.as_deref());
            }
            _ => panic!("expected Create variant"),
        }
        let cli = Cli::parse_from(["test", "create"]);
        match cli.command {
            WorktreeCommand::Create { name, git_ref } => {
                assert!(name.is_none());
                assert!(git_ref.is_none());
            }
            _ => panic!("expected Create variant"),
        }
    }
    #[test]
    fn ext_request_builds_list_with_filters() {
        let req = ext_request(
            "x.ai/git/worktree/list",
            &serde_json::json!({
                "repo": "xai",
                "type": ["session"],
                "includeAll": true,
            }),
        )
        .unwrap();
        assert_eq!(req.method.as_ref(), "x.ai/git/worktree/list");
        let params: serde_json::Value = serde_json::from_str(req.params.get()).unwrap();
        assert_eq!(params.get("repo").and_then(|v| v.as_str()), Some("xai"));
        assert_eq!(params.get("includeAll"), Some(&serde_json::json!(true)));
    }
    #[test]
    fn ext_request_builds_gc_with_max_age_string() {
        let req = ext_request(
            "x.ai/git/worktree/gc",
            &serde_json::json!({
                "dryRun": true,
                "maxAge": "7d",
                "force": false,
            }),
        )
        .unwrap();
        let params: serde_json::Value = serde_json::from_str(req.params.get()).unwrap();
        assert_eq!(params.get("maxAge").and_then(|v| v.as_str()), Some("7d"));
        assert_eq!(params.get("dryRun"), Some(&serde_json::json!(true)));
    }
    #[test]
    fn ext_request_builds_remove_with_id_or_path() {
        let req = ext_request(
            "x.ai/git/worktree/remove",
            &serde_json::json!({
                "idOrPath": "wt-abc123",
                "force": true,
                "dryRun": false,
            }),
        )
        .unwrap();
        let params: serde_json::Value = serde_json::from_str(req.params.get()).unwrap();
        assert_eq!(
            params.get("idOrPath").and_then(|v| v.as_str()),
            Some("wt-abc123")
        );
    }
    #[test]
    fn ext_request_builds_show() {
        let req = ext_request(
            "x.ai/git/worktree/show",
            &serde_json::json!({ "idOrPath": "/some/path" }),
        )
        .unwrap();
        let params: serde_json::Value = serde_json::from_str(req.params.get()).unwrap();
        assert_eq!(
            params.get("idOrPath").and_then(|v| v.as_str()),
            Some("/some/path")
        );
    }
    #[test]
    fn ext_request_builds_detach_salvage_clean() {
        let d = ext_request(
            "x.ai/git/worktree/detach",
            &serde_json::json!({ "idOrPath": "/wt", "allowCopy": false }),
        )
        .unwrap();
        assert_eq!(d.method.as_ref(), "x.ai/git/worktree/detach");
        let s = ext_request(
            "x.ai/git/worktree/salvage",
            &serde_json::json!({ "idOrPath": "/wt", "out": "/out" }),
        )
        .unwrap();
        assert_eq!(s.method.as_ref(), "x.ai/git/worktree/salvage");
        let c = ext_request(
            "x.ai/git/worktree/clean-artifacts",
            &serde_json::json!({ "idOrPath": "/wt" }),
        )
        .unwrap();
        assert_eq!(c.method.as_ref(), "x.ai/git/worktree/clean-artifacts");
    }
    #[test]
    fn ext_request_builds_db_stats_empty_params() {
        let req = ext_request("x.ai/git/worktree/db/stats", &()).unwrap();
        assert_eq!(req.method.as_ref(), "x.ai/git/worktree/db/stats");
    }
    #[test]
    fn remove_response_deserializes_with_resolved_path() {
        let json = r#"{"removed": true, "resolvedPath": "/resolved"}"#;
        let resp: RemoveResponse = serde_json::from_str(json).unwrap();
        assert!(resp.removed);
        assert_eq!(resp.resolved_path.as_deref(), Some("/resolved"));
    }
    #[test]
    fn remove_response_deserializes_without_resolved_path() {
        let json = r#"{"removed": true}"#;
        let resp: RemoveResponse = serde_json::from_str(json).unwrap();
        assert!(resp.removed);
        assert!(resp.resolved_path.is_none());
    }
    #[test]
    fn ext_envelope_unwraps_success_result() {
        let json = r#"{"result": {"path": "/home/user/.grok/worktrees.db"}, "error": null}"#;
        #[derive(serde::Deserialize)]
        struct PathResp {
            path: String,
        }
        let envelope: ExtEnvelope<PathResp> = serde_json::from_str(json).unwrap();
        assert!(envelope.error.is_none());
        let inner = envelope.result.unwrap();
        assert_eq!(inner.path, "/home/user/.grok/worktrees.db");
    }
    #[test]
    fn ext_envelope_unwraps_error_result() {
        let json = r#"{"result": null, "error": "something went wrong"}"#;
        let envelope: ExtEnvelope<serde_json::Value> = serde_json::from_str(json).unwrap();
        assert!(envelope.result.is_none());
        assert!(envelope.error.is_some());
    }
    #[test]
    fn ext_envelope_unwraps_list_of_records() {
        let json = r#"{"result": [], "error": null}"#;
        let envelope: ExtEnvelope<Vec<WorktreeRecord>> = serde_json::from_str(json).unwrap();
        assert!(envelope.error.is_none());
        assert!(envelope.result.unwrap().is_empty());
    }
    #[test]
    fn ext_envelope_unwraps_db_stats() {
        let json = r#"{"result": {"total_records": 5, "alive_count": 3, "dead_count": 2, "db_file_bytes": 1024}}"#;
        let envelope: ExtEnvelope<DbStats> = serde_json::from_str(json).unwrap();
        let stats = envelope.result.unwrap();
        assert_eq!(stats.total_records, 5);
        assert_eq!(stats.alive_count, 3);
    }
    #[test]
    fn ext_envelope_unwraps_gc_report() {
        let json = r#"{"result": {"dead_removed": 2, "expired_removed": 1, "skipped_alive": 0}}"#;
        let envelope: ExtEnvelope<GcReport> = serde_json::from_str(json).unwrap();
        let report = envelope.result.unwrap();
        assert_eq!(report.dead_removed, 2);
        assert_eq!(report.expired_removed, 1);
        assert_eq!(report.remove_failed, 0);
    }
    /// The GC report prints kept (not reclaimable) worktrees apart from guarded (in use) ones, and non-repository paths apart from removals.
    #[test]
    fn kept_worktree_prints_apart_from_a_busy_one_and_from_a_removal() {
        let json = r#"{"result": {"dead_removed": 0, "expired_removed": 3, "skipped_alive": 0,
            "kept_unsafe": 2, "no_repo_paths": 1, "kept_reasons": {"dirty": 2},
            "kept": [{"path": "/wt", "reason": "dirty"}], "not_judged": 4, "unnamed": 5}}"#;
        let envelope: ExtEnvelope<GcReport> = serde_json::from_str(json).unwrap();
        let mut out = Vec::new();
        display::print_gc(&envelope.result.unwrap(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            [
                "GC report:",
                "  Dead records removed:      0",
                "  Expired worktrees removed: 3",
                "  Non-repository paths:      1",
                "  Skipped (guarded):         0",
                "  Kept (not reclaimable):    2",
                "    dirty: 2",
                "      /wt  (dirty)",
                "      and 1 more, named in the log",
                "  Not judged this pass:      4",
                "  Naming failed (kept):      5",
            ]
        );
    }
    #[test]
    fn rm_parses_short_force_flag() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            command: WorktreeCommand,
        }
        let cli = Cli::parse_from(["test", "rm", "-f", "wt-1"]);
        match cli.command {
            WorktreeCommand::Rm {
                ids,
                force,
                dry_run,
            } => {
                assert!(force);
                assert!(!dry_run);
                assert_eq!(ids, vec!["wt-1"]);
            }
            _ => panic!("expected Rm variant"),
        }
    }
    #[test]
    fn rm_parses_long_force_flag() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            command: WorktreeCommand,
        }
        let cli = Cli::parse_from(["test", "rm", "--force", "a", "b"]);
        match cli.command {
            WorktreeCommand::Rm {
                ids,
                force,
                dry_run,
            } => {
                assert!(force);
                assert!(!dry_run);
                assert_eq!(ids, vec!["a", "b"]);
            }
            _ => panic!("expected Rm variant"),
        }
    }
    #[test]
    fn gc_parses_short_force_flag() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            command: WorktreeCommand,
        }
        let cli = Cli::parse_from(["test", "gc", "-f"]);
        match cli.command {
            WorktreeCommand::Gc {
                force,
                dry_run,
                max_age,
            } => {
                assert!(force);
                assert!(!dry_run);
                assert!(max_age.is_none());
            }
            _ => panic!("expected Gc variant"),
        }
    }
    #[test]
    fn list_accepts_ls_alias() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            command: WorktreeCommand,
        }
        let cli = Cli::parse_from(["test", "ls", "--json"]);
        match cli.command {
            WorktreeCommand::List {
                repo,
                r#type,
                json,
                all,
            } => {
                assert!(repo.is_none());
                assert!(r#type.is_empty());
                assert!(json);
                assert!(!all);
            }
            _ => panic!("expected List variant"),
        }
    }
}
