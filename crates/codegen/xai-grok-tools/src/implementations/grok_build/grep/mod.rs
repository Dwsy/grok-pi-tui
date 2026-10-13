//! `grep` tool — new architecture (`Tool` trait).
//!
//! Wraps ripgrep to search file contents. Reads `Cwd` from Resources and
//! truncation settings from its own `Params<GrepParams>`.
//!
//! The ripgrep binary resolution logic (`rg_path()`) is shared with the
//! old implementation via `implementations::grep::ripgrep`.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use crate::DEFAULT_TOOL_OUTPUT_BYTES;
use crate::computer::content_search::RootKind;
use crate::implementations::grok_build::grep::capped_output::{CappedOutput, Fill};
use crate::implementations::grok_build::grep::card::{
    GrepFormatConfig, finalize_grep, grep_partial_timeout_output, grep_timeout_output,
    spawn_failure_output,
};
use crate::implementations::grok_build::grep::offer::{
    GrepSource, ResolvedSource, content_search_request,
};
use crate::implementations::grok_build::grep::rg_runner::{RgArgs, RgRunner};
use crate::types::output::GrepSearchOutput;
#[allow(unused_imports)]
use crate::types::resources::{
    Cwd, DenyReadGlobs, DisplayCwd, FileSystem, Params, PathNotFoundHints, SharedResources,
    display_cwd_or_cwd, resolve_model_path,
};
use crate::types::tool::{ToolKind, ToolNamespace};

// ───────────────────────────────────────────────────────────────────────────
// Input
// ───────────────────────────────────────────────────────────────────────────

use serde::{Deserialize, Serialize};

mod capped_output;
mod card;
mod offer;
mod rg_heading;
mod rg_runner;
pub mod ripgrep;

pub use crate::implementations::grok_build::grep::card::{
    count_matches, format_content_output, format_count_output, format_files_with_matches_output,
    parse_file_matches, parse_numbered_line_prefix,
};
// Re-export the shared GrokIntegerSchema from types module
pub use crate::types::GrokIntegerSchema;
use ripgrep::rg_path;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    #[default]
    Content,
    FilesWithMatches,
    Count,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GrepSearchInput {
    #[schemars(
        description = "The regular expression pattern to search for in file contents (rg --regexp)"
    )]
    pub pattern: String,

    #[schemars(
        description = "File or directory to search in (rg pattern -- PATH). Defaults to workspace path."
    )]
    pub path: Option<String>,

    #[schemars(
        description = r#"Glob pattern (rg --glob GLOB -- PATH) to filter files (e.g. "*.js", "*.{ts,tsx}")."#
    )]
    pub glob: Option<String>,

    /// Accepted on the wire when present; omitted from the JSON schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub output_mode: Option<OutputMode>,

    #[schemars(
        rename = "-B",
        with = "GrokIntegerSchema",
        description = "Number of lines to show before each match (rg -B)."
    )]
    #[serde(rename = "-B")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_context: Option<usize>,

    #[schemars(
        rename = "-A",
        with = "GrokIntegerSchema",
        description = "Number of lines to show after each match (rg -A)."
    )]
    #[serde(rename = "-A")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_context: Option<usize>,

    #[schemars(
        rename = "-C",
        with = "GrokIntegerSchema",
        description = "Number of lines to show before and after each match (rg -C)."
    )]
    #[serde(rename = "-C")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<usize>,

    #[schemars(rename = "-i", description = "Case insensitive search (rg -i).")]
    #[serde(
        rename = "-i",
        default,
        deserialize_with = "crate::types::schema::deserialize_lenient_bool"
    )]
    pub case_insensitive: bool,

    #[schemars(
        description = "File type to search (rg --type). Common types: js, py, rust, go, java, etc. More efficient than glob for standard file types."
    )]
    pub r#type: Option<String>,

    #[schemars(
        with = "GrokIntegerSchema",
        description = "Limit output to first N lines/entries, equivalent to \"| head -N\". Defaults to 200 lines or 500 entries."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_limit: Option<usize>,

    #[schemars(
        description = "Enable multiline mode where . matches newlines and patterns can span lines (rg -U --multiline-dotall)."
    )]
    #[serde(
        default,
        deserialize_with = "crate::types::schema::deserialize_lenient_bool"
    )]
    pub multiline: bool,
}

// ───────────────────────────────────────────────────────────────────────────
// Params
// ───────────────────────────────────────────────────────────────────────────

/// Per-tool configuration for `grep`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrepParams {
    /// Maximum output size in bytes before truncation.
    /// Defaults to `DEFAULT_TOOL_OUTPUT_BYTES` (40 KB) when `None`.
    pub max_output_bytes: Option<usize>,
    /// Maximum characters per line before truncation.
    /// Defaults to 1000 when `None`.
    pub max_chars_per_line: Option<usize>,
}

crate::register_resource!("grok_build", "Grep", GrepParams);

// ───────────────────────────────────────────────────────────────────────────
// Constants
// ───────────────────────────────────────────────────────────────────────────

/// Hard max when the model passes an explicit `head_limit` (content lines).
const CONTENT_LINE_LIMIT: usize = 2_000;
/// Default when `head_limit` is omitted (content). Chosen near observed agent
/// usage (most explicit limits are 20–50; max useful exploration is well under
/// 2000) so large-repository walks stop earlier than the hard max.
const CONTENT_LINE_DEFAULT: usize = 200;
/// Hard max for files_with_matches / count entry lists.
const FILE_COUNT_LIMIT: usize = 10_000;
/// Default when `head_limit` is omitted (files/count modes).
const FILE_COUNT_DEFAULT: usize = 500;
pub const DEFAULT_MAX_CHARS_PER_LINE: usize = 1_000;

/// Default grep wall-clock timeout (seconds) on non-WSL platforms.
const GREP_TIMEOUT_DEFAULT_SECS: u64 = 20;

/// Grep wall-clock timeout (seconds) under WSL, where filesystem reads are 3-5x slower.
const GREP_TIMEOUT_WSL_SECS: u64 = 60;

/// Grep's wall-clock timeout in whole seconds: 60s on WSL (slow filesystem), 20s elsewhere.
fn grep_timeout_secs(is_wsl: bool) -> u64 {
    if is_wsl {
        GREP_TIMEOUT_WSL_SECS
    } else {
        GREP_TIMEOUT_DEFAULT_SECS
    }
}

/// Grep's wall-clock timeout for the current platform.
fn grep_timeout() -> Duration {
    Duration::from_secs(grep_timeout_secs(xai_tty_utils::is_wsl()))
}

/// Resolve the effective line/entry budget for this call. Always returns a finite limit so we can
/// stop reading (and kill `rg`) once enough output is in hand — even when the model omits
/// `head_limit`.
fn resolve_effective_head_limit(input: &GrepSearchInput, output_mode: &OutputMode) -> usize {
    let (default, cap) = match output_mode {
        OutputMode::Content => (CONTENT_LINE_DEFAULT, CONTENT_LINE_LIMIT),
        OutputMode::FilesWithMatches | OutputMode::Count => (FILE_COUNT_DEFAULT, FILE_COUNT_LIMIT),
    };
    input.head_limit.unwrap_or(default).min(cap)
}

/// Hard `head_limit` ceiling for a mode (what an explicit limit is clamped to). Callers that
/// paginate over the full underlying result themselves must request this instead of `head_limit:
/// None`, which now resolves to the small omitted-`head_limit` default and kills `rg` early.
pub fn max_head_limit(output_mode: &OutputMode) -> usize {
    match output_mode {
        OutputMode::Content => CONTENT_LINE_LIMIT,
        OutputMode::FilesWithMatches | OutputMode::Count => FILE_COUNT_LIMIT,
    }
}

/// grep's capabilities incl. its streaming spec (single source of truth). grep streams the formatted card body
/// (`PlainText` / `Append`), never raw stdout; the `<workspace_result …>` wrapper and "Found N …" summary are a
/// terminal-only footer, so the stream is a faithful prefix of the card body.
static GREP_CAPABILITIES: LazyLock<xai_tool_protocol::ToolCapabilities> =
    LazyLock::new(|| xai_tool_protocol::ToolCapabilities {
        is_read_only: true,
        tool_scope: Some(xai_tool_protocol::ToolScope::Read),
        streaming: Some(xai_tool_protocol::StreamingSpec {
            subkind: "grep_match_chunk".to_owned(),
            max_delta_bytes: None,
        }),
        ..Default::default()
    });

// ───────────────────────────────────────────────────────────────────────────
// Tool implementation
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct GrepTool;

impl crate::types::tool_metadata::ToolMetadata for GrepTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Search
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn lock_path_param(&self) -> Option<&'static str> {
        Some("path")
    }

    fn description_template(&self) -> &str {
        r#"Search file contents with regular expressions (ripgrep).

- Full regex syntax, so escape literal special characters: `functionCall\(`, or `interface\{\}` to find interface{} in Go.
- Pass ${{ params.search.pattern }} as a raw regex string — no surrounding quotes.
- Respects .gitignore unless you pass a broad glob like '--glob *'.
- Only filter by '${{ params.search.type }}' or '${{ params.search.glob }}' when you are sure of the file type; import paths may not match source file types (.js vs .ts).
- Output is ripgrep-style: ':' marks match lines, '-' marks context lines, grouped by file. Large results are capped and report "at least" counts."#
    }
}

impl xai_tool_runtime::Tool for GrepTool {
    type Args = GrepSearchInput;
    type Output = GrepSearchOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new("grep").expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            "grep",
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        // Clone of `GREP_CAPABILITIES`; read at registration time only.
        GREP_CAPABILITIES.clone()
    }

    /// Streaming entry point. Gate OFF (default): byte-for-byte the blocking [`GrepTool::run`] contract. Gate ON: spawn ripgrep, project each match
    /// line via [`BodyStreamer`] (same projection [`finalize_grep`] re-derives in batch) and emit `grep_match_chunk` deltas — the stream is a
    /// faithful prefix of the terminal card body. Gated by `WorkspaceViewerContext::stream_tool_progress`.
    async fn execute(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: GrepSearchInput,
    ) -> xai_tool_runtime::ToolStream<GrepSearchOutput> {
        // Absent extension or spec ⇒ gate off. `Some(spec)` iff the gate is
        // on; the spec borrow is `'static` (LazyLock), so it moves straight
        // into the stream below.
        let admitted_spec = ctx
            .get::<xai_tool_runtime::WorkspaceViewerContext>()
            .zip(GREP_CAPABILITIES.streaming.as_ref())
            .filter(|(vctx, _)| vctx.stream_tool_progress)
            .map(|(_, spec)| spec);

        // Fast path: gate off ⇒ run the blocking implementation and wrap its
        // single result. Identical to the pre-streaming contract.
        let Some(spec) = admitted_spec else {
            return xai_tool_runtime::terminal_only(self.run(ctx, input).await);
        };

        // `tool.grep` span matching `run`'s; a guard can't be held across
        // the stream's await points, so the handle is scoped explicitly.
        let span = tracing::info_span!(
            "tool.grep",
            timed_out = tracing::field::Empty,
            wall_ms = tracing::field::Empty,
            early_kill = tracing::field::Empty,
            effective_head_limit = tracing::field::Empty,
            grep_reason = tracing::field::Empty,
            answered_by = tracing::field::Empty,
            served_failure = tracing::field::Empty,
        );

        grep_progress_stream(ctx, input, spec, span)
    }

    #[tracing::instrument(
        name = "tool.grep",
        skip_all,
        fields(
            timed_out = tracing::field::Empty,
            wall_ms = tracing::field::Empty,
            early_kill = tracing::field::Empty,
            effective_head_limit = tracing::field::Empty,
            grep_reason = tracing::field::Empty,
            answered_by = tracing::field::Empty,
            served_failure = tracing::field::Empty,
        )
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: GrepSearchInput,
    ) -> Result<GrepSearchOutput, xai_tool_runtime::ToolError> {
        let started = std::time::Instant::now();
        let early_return = |out: GrepSearchOutput| {
            tracing::Span::current().record("wall_ms", started.elapsed().as_millis() as u64);
            tracing::Span::current().record("early_kill", false);
            // Negative Early exit is the spawn-failure arm, not an unclassified -1.
            if out.exit_code < 0 {
                tracing::Span::current().record("grep_reason", "spawn_failure");
            }
            out
        };
        let GrepReady { source, config } = match prepare_grep(&ctx, &input).await? {
            GrepStep::Ready(ready) => ready,
            GrepStep::Early(out) => return Ok(early_return(out)),
        };
        tracing::Span::current().record("effective_head_limit", config.effective_head_limit as u64);
        let (mut rg, deadline) = match source.resolve(&config, &tracing::Span::current()).await {
            ResolvedSource::Rg { rg, deadline } => (rg, deadline),
            ResolvedSource::Answered(answer) => {
                let (_, output) = answer.into_card(CappedOutput::new(&config), &config);
                let wall_ms = started.elapsed().as_millis() as u64;
                tracing::Span::current().record("wall_ms", wall_ms);
                tracing::Span::current().record("early_kill", false);
                tracing::info!(
                    wall_ms,
                    early_kill = false,
                    effective_head_limit = config.effective_head_limit,
                    exit_code = output.exit_code,
                    "grep finished"
                );
                return Ok(output);
            }
            ResolvedSource::SpawnFailed(out) => return Ok(early_return(out)),
        };

        let timeout = grep_timeout();
        let mut output = CappedOutput::new(&config);
        let io_result = tokio::time::timeout_at(deadline, async {
            let stdout_truncated = loop {
                let Some(chunk) = rg.read_stdout().await else {
                    break false;
                };
                match output.accept(chunk).fill {
                    Fill::Open => {}
                    Fill::Overflowed => break true,
                    Fill::FullAtChunkEnd => break rg.has_more_stdout().await,
                }
            };
            if stdout_truncated {
                rg.stop();
            }
            let mut stderr_buf = Vec::new();
            rg.drain_stderr(&mut stderr_buf).await;
            (stdout_truncated, stderr_buf)
        })
        .await;

        let (stdout_truncated, stderr_buf) = match io_result {
            Ok(result) => result,
            Err(_elapsed) => {
                tracing::Span::current().record("timed_out", true);
                tracing::Span::current().record("early_kill", true);
                tracing::Span::current().record("grep_reason", "timeout");
                tracing::Span::current().record("wall_ms", started.elapsed().as_millis() as u64);
                tracing::warn!(timeout_secs = timeout.as_secs(), "grep timed out");
                rg.kill_and_reap().await;
                return Ok(grep_timeout_output(timeout.as_secs()));
            }
        };

        let exit_code = rg.exit_code(stdout_truncated).await;

        tracing::Span::current().record("early_kill", stdout_truncated);
        if stdout_truncated {
            tracing::Span::current().record("grep_reason", "early_stop");
        }
        tracing::Span::current().record("wall_ms", started.elapsed().as_millis() as u64);
        tracing::info!(
            wall_ms = started.elapsed().as_millis() as u64,
            early_kill = stdout_truncated,
            effective_head_limit = config.effective_head_limit,
            exit_code,
            "grep finished"
        );

        Ok(finalize_grep(
            output.into_bytes(),
            stdout_truncated,
            stderr_buf,
            exit_code,
            &config,
        ))
    }
}

/// Streaming grep pipeline: spawn ripgrep, project each match line via
/// `BodyStreamer`, and emit deltas before the terminal card.
fn grep_progress_stream(
    ctx: xai_tool_runtime::ToolCallContext,
    input: GrepSearchInput,
    spec: &'static xai_tool_protocol::StreamingSpec,
    span: tracing::Span,
) -> xai_tool_runtime::ToolStream<GrepSearchOutput> {
    Box::pin(async_stream::stream! {
        let stream_started = std::time::Instant::now();
        // Mirror `run`'s Early arm so path-not-found / spawn short-circuits
        // still populate the `tool.grep` span in the streaming (prod) path.
        let record_early = |out: &GrepSearchOutput| {
            span.record("wall_ms", stream_started.elapsed().as_millis() as u64);
            span.record("early_kill", false);
            if out.exit_code < 0 {
                span.record("grep_reason", "spawn_failure");
            }
        };
        let GrepReady { source, config } = match prepare_grep(&ctx, &input).await {
            Ok(GrepStep::Ready(ready)) => ready,
            Ok(GrepStep::Early(out)) => {
                record_early(&out);
                yield xai_tool_runtime::ToolStreamItem::Terminal(Ok(out));
                return;
            }
            Err(e) => {
                yield xai_tool_runtime::ToolStreamItem::Terminal(Err(e));
                return;
            }
        };

        span.record("effective_head_limit", config.effective_head_limit as u64);
        let (mut rg, deadline_at) = match source.resolve(&config, &span).await {
            ResolvedSource::Rg { rg, deadline } => (rg, deadline),
            ResolvedSource::Answered(answer) => {
                let (deltas, output) =
                    answer.into_card(CappedOutput::streaming(spec, &config), &config);
                for p in deltas {
                    yield xai_tool_runtime::ToolStreamItem::Progress(p);
                }
                let wall_ms = stream_started.elapsed().as_millis() as u64;
                span.record("wall_ms", wall_ms);
                span.record("early_kill", false);
                span.in_scope(|| {
                    tracing::info!(
                        wall_ms,
                        early_kill = false,
                        effective_head_limit = config.effective_head_limit,
                        exit_code = output.exit_code,
                        "grep finished"
                    );
                });
                yield xai_tool_runtime::ToolStreamItem::Terminal(Ok(output));
                return;
            }
            ResolvedSource::SpawnFailed(out) => {
                record_early(&out);
                yield xai_tool_runtime::ToolStreamItem::Terminal(Ok(out));
                return;
            }
        };
        // Raw bytes for the authoritative terminal card, plus the incremental
        // card-body formatter (deltas == terminal body).
        let mut output = CappedOutput::streaming(spec, &config);
        let mut timed_out = false;
        // One deadline shared by stdout loop + stderr drain (same total
        // budget as `run`).
        let timeout = grep_timeout();
        // Deadline rides the `select!` (can't wrap a yielding block).
        let deadline = tokio::time::sleep_until(deadline_at);
        tokio::pin!(deadline);
        let stdout_truncated = loop {
            let chunk = tokio::select! {
                biased;
                _ = &mut deadline => {
                    timed_out = true;
                    break false;
                }
                chunk = rg.read_stdout() => chunk,
            };
            let Some(chunk) = chunk else {
                break false;
            };
            let accepted = output.accept(chunk);
            for p in accepted.deltas {
                yield xai_tool_runtime::ToolStreamItem::Progress(p);
            }
            match accepted.fill {
                Fill::Open => {}
                Fill::Overflowed => break true,
                // Probe budget only, never the shared `deadline_at`: expiring the tool deadline here would force the
                // timeout terminal branch (banner, exit -1) for a normal head-limit fill near the wall-clock edge.
                Fill::FullAtChunkEnd => break rg.has_more_stdout().await,
            }
        };

        if timed_out {
            span.record("timed_out", true);
            span.record("early_kill", true);
            span.record("grep_reason", "timeout");
            span.record("wall_ms", stream_started.elapsed().as_millis() as u64);
            let secs = timeout.as_secs();
            span.in_scope(|| {
                tracing::warn!(timeout_secs = secs, "grep timed out");
            });
            rg.kill_and_reap().await;
            if let Some(p) = output.finish() {
                yield xai_tool_runtime::ToolStreamItem::Progress(p);
            }
            let timed_out_output = grep_partial_timeout_output(output.into_bytes(), secs, &config);
            yield xai_tool_runtime::ToolStreamItem::Terminal(Ok(timed_out_output));
            return;
        }
        span.record("timed_out", false);

        // Flush the final non-terminated segment (see `BodyStreamer::finish`).
        if let Some(p) = output.finish() {
            yield xai_tool_runtime::ToolStreamItem::Progress(p);
        }

        // stderr is small and never streamed; still bounded by the shared
        // deadline as a backstop so a wedged child can't stall the stream.
        if stdout_truncated {
            rg.stop();
        }
        let mut stderr_buf = Vec::new();
        if tokio::time::timeout_at(deadline_at, rg.drain_stderr(&mut stderr_buf))
        .await
        .is_err()
        {
            span.in_scope(|| tracing::debug!("rg stderr drain hit the grep deadline"));
        }

        let exit_code = rg.exit_code(stdout_truncated).await;

        let wall_ms = stream_started.elapsed().as_millis() as u64;
        span.record("early_kill", stdout_truncated);
        if stdout_truncated {
            span.record("grep_reason", "early_stop");
        }
        span.record("wall_ms", wall_ms);
        span.in_scope(|| {
            tracing::info!(
                wall_ms,
                early_kill = stdout_truncated,
                effective_head_limit = config.effective_head_limit,
                exit_code,
                "grep finished"
            );
        });

        let output =
            finalize_grep(output.into_bytes(), stdout_truncated, stderr_buf, exit_code, &config);
        yield xai_tool_runtime::ToolStreamItem::Terminal(Ok(output));
    })
}

/// A spawned ripgrep or an offered search, plus the resolved formatting config.
struct GrepReady {
    source: GrepSource,
    config: GrepFormatConfig,
}

/// Outcome of [`prepare_grep`]: either a source to read, or a fully
/// formed early result (path-not-found / spawn failure) that needs no reading.
#[allow(clippy::large_enum_variant)]
enum GrepStep {
    Ready(GrepReady),
    Early(GrepSearchOutput),
}

/// Resolve resources, then take the file system's offer or spawn ripgrep; `Early`
/// for pre-read short-circuits. Shared by `run` and `execute`.
async fn prepare_grep(
    ctx: &xai_tool_runtime::ToolCallContext,
    input: &GrepSearchInput,
) -> Result<GrepStep, xai_tool_runtime::ToolError> {
    use crate::types::tool_metadata::{resolve_cwd, shared_resources};
    let resources = shared_resources(ctx)?;
    let cwd = resolve_cwd(ctx, &resources).await?;
    let (display_cwd, hints_enabled, deny_read_globs, file_system) = {
        let res = resources.lock().await;
        (
            res.get::<DisplayCwd>().map(|d| d.0.clone()),
            res.get::<PathNotFoundHints>().is_some_and(|h| h.0),
            res.get::<DenyReadGlobs>()
                .map(|d| d.0.clone())
                .unwrap_or_default(),
            res.get::<FileSystem>().map(|fs| Arc::clone(&fs.0)),
        )
    };

    // Resolve the model-provided path for the working directory.
    let workdir = resolve_model_path(
        &cwd,
        display_cwd.as_deref(),
        input.path.as_deref().unwrap_or(""),
    );
    // Use display_cwd for output paths so model sees stable paths.
    let display_base = display_cwd_or_cwd(&cwd, display_cwd.as_deref());
    let cwd_display = display_base.display().to_string();
    if let Err(error) = crate::types::memory_v2::validate_memory_v2_read(&resources, &workdir).await
    {
        return Ok(GrepStep::Early(GrepSearchOutput {
            stdout: Vec::new(),
            stderr: error.into_bytes(),
            exit_code: 2,
            match_count: 0,
            file_matches: Vec::new(),
        }));
    }

    // A content search offer also needs to know what the root is.
    let root_metadata = if input.path.is_some() || file_system.is_some() {
        Some(tokio::fs::metadata(&workdir).await)
    } else {
        None
    };
    // Pre-check: if the search path doesn't exist, return enriched hints before rg runs. We intentionally pre-check with metadata() rather than
    // parsing rg's stderr after the fact because rg lumps all errors under exit code 2 (path not found, invalid regex, bad glob, unknown file
    // type, etc.). Distinguishing path-not-found would require matching on OS error strings in stderr, which is fragile.
    if input.path.is_some()
        && let Some(Err(e)) = &root_metadata
        && e.kind() == std::io::ErrorKind::NotFound
    {
        let display_path = if let Ok(suffix) = workdir.strip_prefix(&cwd) {
            display_base.join(suffix)
        } else {
            workdir.clone()
        };
        let msg = crate::util::format_not_found_error(
            &display_path,
            &workdir,
            &cwd,
            &display_base,
            hints_enabled,
        )
        .await;
        return Ok(GrepStep::Early(GrepSearchOutput {
            stdout: msg.into_bytes(),
            stderr: Vec::new(),
            exit_code: 2,
            match_count: 0,
            file_matches: Vec::new(),
        }));
    }

    let root_kind = match &root_metadata {
        Some(Ok(metadata)) if metadata.is_dir() => RootKind::Directory,
        Some(Ok(metadata)) if metadata.is_file() => RootKind::File,
        Some(Ok(_) | Err(_)) | None => RootKind::Unknown,
    };

    let output_mode = input.output_mode.clone().unwrap_or(OutputMode::Content);
    let effective_head_limit = resolve_effective_head_limit(input, &output_mode);

    // Managed Read-deny globs become ripgrep excludes so a search never reads a policy-forbidden path — whether reached by a recursive walk or by
    // a `glob` arg that targets a denied file. Added AFTER the caller's `--glob` so the exclude wins (ripgrep applies the last matching glob). An
    // explicitly-passed denied `path` is blocked earlier by the permission manager (ripgrep searches explicit paths even against excludes).
    let globs: Vec<String> = input
        .glob
        .iter()
        .filter(|glob| !glob.is_empty())
        .cloned()
        .chain(deny_read_globs.iter().map(|deny| format!("!{deny}")))
        .collect();
    let args = RgArgs {
        program: rg_path()?,
        input,
        output_mode: &output_mode,
        root: &workdir,
        globs: &globs,
    };
    let offer = file_system.and_then(|file_system| {
        let request = content_search_request(&args, root_kind, effective_head_limit)?;
        let job = file_system.offer_content_search(&request)?;
        Some((job, request))
    });
    let rg_command = args.into_command();
    let source = match offer {
        Some((job, request)) => GrepSource::Offered {
            job,
            request,
            rg_command,
        },
        None => match RgRunner::spawn(rg_command) {
            Ok(rg) => GrepSource::Rg(rg),
            Err(error) => return Ok(GrepStep::Early(spawn_failure_output(&error))),
        },
    };

    // Resolve truncation settings from tool-specific Params (static config; no
    // dependency on the rg output, so it is resolved up front).
    let params = resources
        .lock()
        .await
        .get::<Params<GrepParams>>()
        .cloned()
        .unwrap_or_default();
    let max_chars_per_line = params
        .0
        .max_chars_per_line
        .unwrap_or(DEFAULT_MAX_CHARS_PER_LINE);
    let max_output_bytes = params
        .0
        .max_output_bytes
        .unwrap_or(DEFAULT_TOOL_OUTPUT_BYTES);

    Ok(GrepStep::Ready(GrepReady {
        source,
        config: GrepFormatConfig {
            output_mode,
            effective_head_limit,
            max_chars_per_line,
            max_output_bytes,
            cwd_display,
        },
    }))
}

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tool_metadata::test_ctx;

    use crate::types::resources::Resources;
    use std::fs;
    use tempfile::TempDir;

    pub(crate) fn make_grep_input(pattern: &str) -> GrepSearchInput {
        GrepSearchInput {
            pattern: pattern.to_string(),
            path: None,
            glob: None,
            output_mode: None,
            before_context: None,
            after_context: None,
            context: None,
            case_insensitive: false,
            r#type: None,
            head_limit: None,
            multiline: false,
        }
    }

    /// Boolean flags must be non-optional in the model-facing schema so the
    /// default is unambiguous (`false`, not `null` + "Default: false" prose).
    #[test]
    fn grep_bool_flags_schema_is_plain_boolean_with_default_false() {
        let schema = serde_json::to_value(schemars::schema_for!(GrepSearchInput)).unwrap();
        let Some(props) = schema.get("properties") else {
            panic!("schema missing properties: {schema}");
        };

        // Field is renamed to "-i" for the model-facing name.
        let Some(case) = props.get("-i") else {
            panic!("schema missing -i: {props}");
        };
        assert_eq!(
            case.get("type").and_then(|v| v.as_str()),
            Some("boolean"),
            "case_insensitive schema: {case}"
        );
        assert_eq!(
            case.get("default").and_then(|v| v.as_bool()),
            Some(false),
            "case_insensitive schema: {case}"
        );
        assert!(
            case.get("anyOf").is_none(),
            "must not use nullable anyOf: {case}"
        );

        let Some(multi) = props.get("multiline") else {
            panic!("schema missing multiline: {props}");
        };
        assert_eq!(
            multi.get("type").and_then(|v| v.as_str()),
            Some("boolean"),
            "multiline schema: {multi}"
        );
        assert_eq!(
            multi.get("default").and_then(|v| v.as_bool()),
            Some(false),
            "multiline schema: {multi}"
        );
        assert!(
            multi.get("anyOf").is_none(),
            "must not use nullable anyOf: {multi}"
        );
    }

    #[test]
    fn grep_bool_flags_deserialize_missing_and_null_as_false() {
        let missing: GrepSearchInput = serde_json::from_str(r#"{"pattern":"foo"}"#).unwrap();
        assert!(!missing.case_insensitive);
        assert!(!missing.multiline);

        let nulls: GrepSearchInput =
            serde_json::from_str(r#"{"pattern":"foo","-i":null,"multiline":null}"#).unwrap();
        assert!(!nulls.case_insensitive);
        assert!(!nulls.multiline);

        let truths: GrepSearchInput =
            serde_json::from_str(r#"{"pattern":"foo","-i":"yes","multiline":1}"#).unwrap();
        assert!(truths.case_insensitive);
        assert!(truths.multiline);
    }

    #[test]
    fn grep_timeout_secs_platform_defaults() {
        assert_eq!(grep_timeout_secs(false), 20);
        assert_eq!(grep_timeout_secs(true), 60);
    }

    #[test]
    fn grep_search_input_schema_omits_output_mode() {
        let schema = crate::registry::types::generate_schema::<GrepSearchInput>();
        let props = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("schema has properties");
        assert!(!props.contains_key("output_mode"));
    }

    #[test]
    fn grep_search_input_deserializes_output_mode_variants() {
        let content: GrepSearchInput =
            serde_json::from_value(serde_json::json!({"pattern": "x", "output_mode": "content"}))
                .unwrap();
        assert_eq!(content.output_mode, Some(OutputMode::Content));

        let files: GrepSearchInput = serde_json::from_value(serde_json::json!({
            "pattern": "x",
            "output_mode": "files_with_matches"
        }))
        .unwrap();
        assert_eq!(files.output_mode, Some(OutputMode::FilesWithMatches));

        let count: GrepSearchInput =
            serde_json::from_value(serde_json::json!({"pattern": "x", "output_mode": "count"}))
                .unwrap();
        assert_eq!(count.output_mode, Some(OutputMode::Count));

        let omitted: GrepSearchInput =
            serde_json::from_value(serde_json::json!({"pattern": "x"})).unwrap();
        assert_eq!(omitted.output_mode, None);
    }

    #[test]
    fn tool_name_and_description() {
        let tool = GrepTool;
        assert_eq!(xai_tool_runtime::Tool::id(&tool).as_str(), "grep");
    }

    #[test]
    fn description_template_tracks_renamed_search_params() {
        use crate::types::template_renderer::TemplateRenderer;
        use crate::types::tool::ToolKind;
        use crate::types::tool_metadata::ToolMetadata;
        use std::collections::HashMap;

        let tools = HashMap::from([(ToolKind::Search, "grep".to_string())]);
        let params = HashMap::from([(
            ToolKind::Search,
            HashMap::from([
                ("pattern".to_string(), "query".to_string()),
                ("type".to_string(), "filetype".to_string()),
                ("glob".to_string(), "include".to_string()),
            ]),
        )]);
        let rendered = TemplateRenderer::new(tools, params)
            .render(ToolMetadata::description_template(&GrepTool))
            .unwrap();
        assert!(
            rendered.contains("'filetype'") && rendered.contains("'include'"),
            "renamed search params must appear:\n{rendered}"
        );
        assert!(
            !rendered.contains("'type'") && !rendered.contains("'glob'"),
            "canonical search param names must not remain after rename:\n{rendered}"
        );
    }

    #[tokio::test]
    async fn tool_grep_no_matches() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("hello.txt"), "hello world\n").unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));

        let tool = GrepTool;
        let output = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            make_grep_input("nonexistent_xyz_pattern"),
        )
        .await
        .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("No matches found"));
        assert_eq!(output.match_count, 0);
    }

    #[tokio::test]
    async fn tool_grep_finds_matches() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("test.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));

        let tool = GrepTool;
        let output = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            make_grep_input("main"),
        )
        .await
        .unwrap();

        assert_eq!(output.exit_code, 0);
        assert!(output.match_count > 0);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("main"));
    }

    /// `DenyReadGlobs` become ripgrep excludes so a search can't read a
    /// read-denied path — neither via a recursive walk (non-dotfile secrets like
    /// `key.pem`) nor via a `glob` arg that targets a denied file.
    #[tokio::test]
    async fn deny_read_globs_exclude_denied_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".env"), "FAKE_SECRET=zzz\n").unwrap();
        fs::write(tmp.path().join("key.pem"), "FAKE in pem\n").unwrap();
        fs::write(tmp.path().join("README.md"), "FAKE in readme\n").unwrap();

        let build = |glob: Option<&str>, deny: &[&str]| {
            let mut resources = Resources::new();
            resources.insert(Cwd(tmp.path().to_path_buf()));
            resources.insert(DenyReadGlobs(deny.iter().map(|s| s.to_string()).collect()));
            let mut input = make_grep_input("FAKE");
            input.glob = glob.map(str::to_string);
            (resources, input)
        };
        let run = |resources: Resources, input| async {
            let out =
                xai_tool_runtime::Tool::run(&GrepTool, test_ctx(resources.into_shared()), input)
                    .await
                    .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };

        // Recursive walk: the read-denied key.pem (a non-dotfile a plain grep
        // would read) is excluded, while a non-denied file still matches.
        let (r, i) = build(None, &["**/*.pem", "**/.env"]);
        let out = run(r, i).await;
        assert!(
            out.contains("README.md"),
            "non-denied file must match: {out}"
        );
        assert!(
            !out.contains("key.pem"),
            "deny glob must exclude key.pem: {out}"
        );

        // A `glob` arg can't target a denied file: the deny exclude overrides it.
        let (r, i) = build(Some(".env"), &["**/.env"]);
        let out = run(r, i).await;
        assert!(
            !out.contains("FAKE_SECRET"),
            "deny glob must beat glob-arg .env: {out}"
        );

        // Control: the same glob arg DOES read .env when no deny globs apply.
        let (r, i) = build(Some(".env"), &[]);
        let out = run(r, i).await;
        assert!(
            out.contains("FAKE_SECRET"),
            "control: glob arg reads .env w/o deny: {out}"
        );
    }

    /// Over-block: deny globs must spare look-alikes whose names resemble a
    /// denied pattern but don't match it (`server.pem.bak`, `notes.env.md`, …).
    #[tokio::test]
    async fn deny_read_globs_spare_glob_boundary_lookalikes() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        // Real denied secrets (non-dotfiles, so a plain grep would read them).
        fs::write(tmp.path().join("src/server.pem"), "FAKE\n").unwrap();
        fs::write(tmp.path().join("terraform.tfstate"), "FAKE\n").unwrap();
        // Look-alikes that must NOT match any deny glob.
        fs::write(tmp.path().join("server.pem.bak"), "FAKE\n").unwrap();
        fs::write(tmp.path().join("notes.env.md"), "FAKE\n").unwrap();
        fs::write(tmp.path().join("env.sample"), "FAKE\n").unwrap();
        fs::write(tmp.path().join("keystore.txt"), "FAKE\n").unwrap();

        let deny = [
            "**/.env",
            "**/.env.*",
            "**/*.pem",
            "**/*.key",
            "**/*.keystore",
            "**/terraform.tfstate",
        ];
        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));
        resources.insert(DenyReadGlobs(deny.iter().map(|s| s.to_string()).collect()));
        let out = String::from_utf8_lossy(
            &xai_tool_runtime::Tool::run(
                &GrepTool,
                test_ctx(resources.into_shared()),
                make_grep_input("FAKE"),
            )
            .await
            .unwrap()
            .stdout,
        )
        .into_owned();

        // Denied secrets stay excluded.
        assert!(
            !out.contains("src/server.pem"),
            "deny must exclude src/server.pem: {out}"
        );
        assert!(
            !out.contains("terraform.tfstate"),
            "deny must exclude terraform.tfstate: {out}"
        );
        // Look-alikes must not be over-blocked.
        for legit in [
            "server.pem.bak",
            "notes.env.md",
            "env.sample",
            "keystore.txt",
        ] {
            assert!(
                out.contains(legit),
                "look-alike `{legit}` wrongly over-blocked: {out}"
            );
        }
    }

    /// Subdir leak: a `**/` deny glob must still exclude a denied file when the
    /// search root is a subdirectory, not just the cwd.
    #[tokio::test]
    async fn deny_read_globs_exclude_in_subdir_search_root() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/server.pem"), "FAKE\n").unwrap();
        fs::write(tmp.path().join("src/main.rs"), "FAKE\n").unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));
        resources.insert(DenyReadGlobs(vec!["**/*.pem".to_string()]));
        let mut input = make_grep_input("FAKE");
        input.path = Some("src".to_string());
        let out = String::from_utf8_lossy(
            &xai_tool_runtime::Tool::run(&GrepTool, test_ctx(resources.into_shared()), input)
                .await
                .unwrap()
                .stdout,
        )
        .into_owned();

        assert!(
            out.contains("main.rs"),
            "legit subdir file must match: {out}"
        );
        assert!(
            !out.contains("server.pem"),
            "deny glob must exclude in subdir search root: {out}"
        );
    }

    /// No-deny regression: a user with no Read-deny rules injects no `DenyReadGlobs`,
    /// so the grep tool excludes nothing — even secret-looking files (`*.pem`) stay
    /// searchable. Guards against over-blocking unrestricted users.
    #[tokio::test]
    async fn no_deny_globs_does_not_block_denied_looking_files() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/server.pem"), "FAKE\n").unwrap();
        fs::write(tmp.path().join("README.md"), "FAKE\n").unwrap();

        // No DenyReadGlobs resource inserted — the no-deny-list user.
        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));
        let out = String::from_utf8_lossy(
            &xai_tool_runtime::Tool::run(
                &GrepTool,
                test_ctx(resources.into_shared()),
                make_grep_input("FAKE"),
            )
            .await
            .unwrap()
            .stdout,
        )
        .into_owned();

        assert!(
            out.contains("server.pem"),
            "no deny list must not exclude server.pem: {out}"
        );
        assert!(
            out.contains("README.md"),
            "non-denied file must match: {out}"
        );
    }

    #[tokio::test]
    async fn tool_works_through_runtime_trait() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("file.txt"), "findme here\n").unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));

        let tool = GrepTool;
        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            make_grep_input("findme"),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.match_count > 0);
    }

    #[tokio::test]
    async fn tool_uses_params_for_truncation() {
        let tmp = TempDir::new().unwrap();
        // Create a file with many matching lines
        let content: String = (0..100).map(|i| format!("match_line_{}\n", i)).collect();
        fs::write(tmp.path().join("big.txt"), &content).unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));
        // Set a very small output limit
        resources.insert(Params(GrepParams {
            max_output_bytes: Some(200),
            max_chars_per_line: None,
        }));

        let tool = GrepTool;
        let output = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            make_grep_input("match_line"),
        )
        .await
        .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("truncated"),
            "output should be truncated with small max_output_bytes, got: {}",
            stdout
        );
    }

    /// Explicit `head_limit` truncates the card and stops the search early.
    #[tokio::test]
    async fn tool_grep_head_limit_truncates() {
        let tmp = TempDir::new().unwrap();
        // Many match lines so an unbounded read would exceed a tiny head_limit.
        let content: String = (0..200).map(|i| format!("findme_{i}\n")).collect();
        fs::write(tmp.path().join("many.txt"), &content).unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));

        let mut input = make_grep_input("findme_");
        input.head_limit = Some(5);

        let output =
            xai_tool_runtime::Tool::run(&GrepTool, test_ctx(resources.into_shared()), input)
                .await
                .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.exit_code, 0);
        assert!(
            stdout.contains("truncated") || output.match_count <= 5,
            "expected head_limit truncation, got: {stdout}"
        );
        // Heading line + a few match lines — well under the full 200 hits.
        let body_lines = stdout.lines().count();
        assert!(
            body_lines < 50,
            "head_limit should keep the card small, got {body_lines} lines: {stdout}"
        );
    }

    /// A result whose rg output-line count exactly equals `head_limit` is complete, not truncated: early-stop reads one line past the budget, so an
    /// exact-fit search reaches EOF without tripping the cap. Regression against the early-stop path over-reporting "at least N" on an exact fit.
    /// (Grouped rg output for one file = 1 heading line + K match lines, so `head_limit = K + 1` is the exact fit.)
    #[tokio::test]
    async fn tool_grep_head_limit_exact_fit_not_truncated() {
        let tmp = TempDir::new().unwrap();
        // One file, exactly 5 matching lines → 6 rg output lines (heading + 5).
        let content: String = (0..5).map(|i| format!("findme_{i}\n")).collect();
        fs::write(tmp.path().join("exact.txt"), &content).unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));

        let mut input = make_grep_input("findme_");
        input.head_limit = Some(6);

        let output =
            xai_tool_runtime::Tool::run(&GrepTool, test_ctx(resources.into_shared()), input)
                .await
                .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.match_count, 5, "all 5 matches counted: {stdout}");
        assert!(
            !stdout.contains("truncated") && !stdout.contains("at least"),
            "exact-fit result must not be marked truncated: {stdout}"
        );
    }

    #[tokio::test]
    async fn tool_grep_files_with_matches_mode() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), "findme\n").unwrap();
        fs::write(tmp.path().join("b.txt"), "findme\n").unwrap();
        fs::write(tmp.path().join("c.txt"), "nothing\n").unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));

        let tool = GrepTool;
        let output = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            GrepSearchInput {
                pattern: "findme".to_string(),
                output_mode: Some(OutputMode::FilesWithMatches),
                ..GrepSearchInput {
                    pattern: String::new(),
                    path: None,
                    glob: None,
                    output_mode: None,
                    before_context: None,
                    after_context: None,
                    context: None,
                    case_insensitive: false,
                    r#type: None,
                    head_limit: None,
                    multiline: false,
                }
            },
        )
        .await
        .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Found 2 files"), "got: {}", stdout);
    }

    #[tokio::test]
    async fn tool_grep_count_mode() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), "findme\nfindme\n").unwrap();
        fs::write(tmp.path().join("b.txt"), "findme\n").unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));

        let tool = GrepTool;
        let output = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            GrepSearchInput {
                pattern: "findme".to_string(),
                output_mode: Some(OutputMode::Count),
                ..GrepSearchInput {
                    pattern: String::new(),
                    path: None,
                    glob: None,
                    output_mode: None,
                    before_context: None,
                    after_context: None,
                    context: None,
                    case_insensitive: false,
                    r#type: None,
                    head_limit: None,
                    multiline: false,
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(output.match_count, 3);
    }

    #[tokio::test]
    async fn tool_grep_with_path_subdir() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("subdir");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("file.txt"), "secret_value\n").unwrap();
        fs::write(tmp.path().join("root.txt"), "other_value\n").unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));

        let tool = GrepTool;
        let output = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            GrepSearchInput {
                pattern: "secret_value".to_string(),
                path: Some("subdir".to_string()),
                glob: None,
                output_mode: None,
                before_context: None,
                after_context: None,
                context: None,
                case_insensitive: false,
                r#type: None,
                head_limit: None,
                multiline: false,
            },
        )
        .await
        .unwrap();

        assert!(output.match_count > 0);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("secret_value"));
    }

    // ─── Streaming (GrepTool::execute) tests ─── `test_ctx` stamps `WorkspaceViewerContext {
    // stream_tool_progress: true }`, so these exercise the streaming path.

    /// Destructure a `grep_match_chunk` payload, asserting the canonical
    /// `plain_text` / `append` envelope. Returns the `delta`.
    pub(crate) fn read_grep_delta(p: &xai_tool_runtime::ToolProgress) -> String {
        match p {
            xai_tool_runtime::ToolProgress::Custom { subkind, payload } => {
                assert_eq!(subkind, "grep_match_chunk", "unexpected subkind");
                payload
                    .get("delta")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("payload missing delta: {payload}"))
                    .to_owned()
            }
            other => panic!("expected Custom progress, got {other:?}"),
        }
    }

    #[test]
    fn resolve_effective_head_limit_defaults_when_omitted() {
        let mut input = make_grep_input("x");
        input.head_limit = None;
        assert_eq!(
            resolve_effective_head_limit(&input, &OutputMode::Content),
            CONTENT_LINE_DEFAULT
        );
        assert_eq!(
            resolve_effective_head_limit(&input, &OutputMode::FilesWithMatches),
            FILE_COUNT_DEFAULT
        );
        input.head_limit = Some(50);
        assert_eq!(
            resolve_effective_head_limit(&input, &OutputMode::Content),
            50
        );
        input.head_limit = Some(CONTENT_LINE_LIMIT + 999);
        assert_eq!(
            resolve_effective_head_limit(&input, &OutputMode::Content),
            CONTENT_LINE_LIMIT
        );
        // Explicit value between default and hard max is honored.
        input.head_limit = Some(800);
        assert_eq!(
            resolve_effective_head_limit(&input, &OutputMode::Content),
            800
        );
    }

    /// Streamed-vs-terminal contract: the concatenation of the per-match-line deltas equals the
    /// terminal card *body* (prefix mode), while the terminal result additionally carries the
    /// `<workspace_result …>` wrapper and the "Found N …" summary (terminal-only footer).
    #[tokio::test]
    async fn grep_streaming_body_matches_card_body() {
        use futures::StreamExt;

        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("matches.txt"),
            "alpha match one\nbeta match two\ngamma match three\n",
        )
        .unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));

        let tool = GrepTool;
        let mut stream = xai_tool_runtime::Tool::execute(
            &tool,
            test_ctx(resources.into_shared()),
            make_grep_input("match"),
        )
        .await;

        let mut deltas = String::new();
        let mut progress = 0usize;
        let mut terminal: Option<Result<GrepSearchOutput, xai_tool_runtime::ToolError>> = None;
        while let Some(item) = stream.next().await {
            match item {
                xai_tool_runtime::ToolStreamItem::Progress(p) => {
                    assert!(terminal.is_none(), "Progress arrived after Terminal");
                    deltas.push_str(&read_grep_delta(&p));
                    progress += 1;
                }
                xai_tool_runtime::ToolStreamItem::Terminal(r) => {
                    assert!(terminal.is_none(), "more than one Terminal yielded");
                    terminal = Some(r);
                }
            }
        }

        assert!(
            progress >= 1,
            "expected at least one grep_match_chunk delta, got {progress}"
        );
        let output = terminal
            .expect("stream ended without a Terminal")
            .expect("grep terminal ok");
        let card = String::from_utf8_lossy(&output.stdout);

        // Terminal card = wrapper + summary + body (the full formatted output).
        assert!(card.starts_with("<workspace_result "), "card: {card}");
        assert!(card.contains("Found 3 matching lines"), "card: {card}");
        assert!(
            card.trim_end().ends_with("</workspace_result>"),
            "card: {card}"
        );

        // The streamed deltas are exactly the card body — the match lines between
        // the summary (line 1) and the closing wrapper (last line) — with neither
        // the wrapper nor the "Found N …" summary.
        let card_lines: Vec<&str> = card.lines().collect();
        let Some(end) = card_lines.len().checked_sub(1) else {
            panic!("card too short: {card_lines:?}");
        };
        let Some(body_lines) = card_lines.get(2..end) else {
            panic!("card missing body: {card_lines:?}");
        };
        let body_from_card = body_lines.join("\n");
        assert_eq!(
            deltas, body_from_card,
            "accumulated deltas must equal the terminal card body"
        );

        // Sanity: the body carries the matches but not the terminal-only footer.
        assert!(deltas.contains("alpha match one"), "deltas: {deltas}");
        assert!(deltas.contains("gamma match three"), "deltas: {deltas}");
        assert!(!deltas.contains("<workspace_result"), "deltas: {deltas}");
        assert!(
            !deltas.contains("Found 3 matching lines"),
            "deltas: {deltas}"
        );
    }

    /// Gate-off invariant: absent `WorkspaceViewerContext`, no Progress is
    /// emitted (byte-for-byte pre-streaming) while the terminal still surfaces.
    #[tokio::test]
    async fn grep_streaming_suppressed_when_gate_off() {
        use futures::StreamExt;

        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("file.txt"), "findme here\n").unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));

        // No streaming gate stamped — exercises the default (gate-off) path.
        let mut ctx = xai_tool_runtime::ToolCallContext::default();
        ctx.extensions.insert(resources.into_shared());

        let tool = GrepTool;
        let mut stream =
            xai_tool_runtime::Tool::execute(&tool, ctx, make_grep_input("findme")).await;

        let mut progress = 0usize;
        let mut terminal: Option<Result<GrepSearchOutput, xai_tool_runtime::ToolError>> = None;
        while let Some(item) = stream.next().await {
            match item {
                xai_tool_runtime::ToolStreamItem::Progress(_) => progress += 1,
                xai_tool_runtime::ToolStreamItem::Terminal(r) => {
                    assert!(terminal.is_none(), "more than one Terminal yielded");
                    terminal = Some(r);
                }
            }
        }

        assert_eq!(
            progress, 0,
            "absent gate must suppress all Progress, got {progress}"
        );
        let output = terminal
            .expect("stream ended without a Terminal")
            .expect("grep terminal ok");
        assert!(
            output.match_count > 0,
            "terminal preserved regardless of gate"
        );
        let card = String::from_utf8_lossy(&output.stdout);
        assert!(card.contains("findme"), "card: {card}");
    }

    /// Streaming invariant under a hit `head_limit`: even when the budget trips mid-stream (so the early-stop / exact-fit
    /// probe path runs), the accumulated deltas must still equal the terminal card body. Regression against feeding the
    /// streamer bytes clobbered by the probe's read.
    #[tokio::test]
    async fn grep_streaming_body_matches_card_body_when_truncated() {
        use futures::StreamExt;

        let tmp = TempDir::new().unwrap();
        // Many matches in one file so the small head_limit is exceeded and the
        // hit_cap / early-stop path is exercised.
        let content: String = (0..200).map(|i| format!("findme_{i}\n")).collect();
        fs::write(tmp.path().join("many.txt"), &content).unwrap();

        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));

        let mut input = make_grep_input("findme_");
        input.head_limit = Some(5);

        let tool = GrepTool;
        let mut stream =
            xai_tool_runtime::Tool::execute(&tool, test_ctx(resources.into_shared()), input).await;

        let mut deltas = String::new();
        let mut terminal: Option<Result<GrepSearchOutput, xai_tool_runtime::ToolError>> = None;
        while let Some(item) = stream.next().await {
            match item {
                xai_tool_runtime::ToolStreamItem::Progress(p) => {
                    assert!(terminal.is_none(), "Progress arrived after Terminal");
                    deltas.push_str(&read_grep_delta(&p));
                }
                xai_tool_runtime::ToolStreamItem::Terminal(r) => {
                    assert!(terminal.is_none(), "more than one Terminal yielded");
                    terminal = Some(r);
                }
            }
        }

        let output = terminal
            .expect("stream ended without a Terminal")
            .expect("grep terminal ok");
        let card = String::from_utf8_lossy(&output.stdout);
        let card_lines: Vec<&str> = card.lines().collect();
        let Some(end) = card_lines.len().checked_sub(1) else {
            panic!("card too short: {card_lines:?}");
        };
        let Some(body_lines) = card_lines.get(2..end) else {
            panic!("card missing body: {card_lines:?}");
        };
        let body_from_card = body_lines.join("\n");
        assert_eq!(
            deltas, body_from_card,
            "accumulated deltas must equal the terminal card body even when truncated"
        );
    }
}
