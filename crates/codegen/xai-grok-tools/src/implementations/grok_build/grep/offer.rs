//! Offering a grep call to the session's file system before `rg` runs.
//!
//! `prepare_grep` describes the `rg` call it would make as a
//! [`ContentSearchRequest`] and asks the `FileSystem` resource for an offer, a
//! pure lookup. [`GrepSource::resolve`] runs an offered job first, bounded by the
//! tool deadline, and renders its hits as `rg --heading` bytes, which then go
//! through the same budgets, streaming and card code as `rg` output. A failed
//! job, a result naming a file `rg` would not search, or a result the card could
//! not show faithfully spawns `rg` with what is left of the request deadline, so
//! a served-then-failed call stays within one grep timeout.

use std::ffi::OsStr;
use std::path::{Component, PathBuf};

use ignore::overrides::OverrideBuilder;
use tokio::process::Command;

use crate::computer::content_search::{
    CaseSensitivity, ContentSearch, ContentSearchJob, ContentSearchMode, ContentSearchOutcome,
    ContentSearchRequest, MultilineMode, RootKind,
};
use crate::implementations::grok_build::grep::capped_output::{
    CappedOutput, Fill, MAX_STDOUT_BYTES,
};
use crate::implementations::grok_build::grep::card::{
    GrepFormatConfig, finalize_grep, grep_partial_timeout_output, spawn_failure_output,
};
use crate::implementations::grok_build::grep::rg_heading::render_rg_heading;
use crate::implementations::grok_build::grep::rg_runner::{
    MAX_COLUMNS, MAX_FILE_BYTES, RgArgs, RgRunner,
};
use crate::implementations::grok_build::grep::{OutputMode, grep_timeout};
use crate::types::output::GrepSearchOutput;

/// Where a grep call's output comes from.
pub(crate) enum GrepSource {
    Rg(RgRunner),
    /// An offered job, with the `rg` command to spawn if it fails.
    Offered {
        job: ContentSearchJob,
        request: ContentSearchRequest,
        rg_command: Command,
    },
}

/// A [`GrepSource`] after the offered job, if any, ran.
pub(crate) enum ResolvedSource {
    /// `rg` answers, and is cut off with a timeout card at `deadline`.
    Rg {
        rg: RgRunner,
        deadline: tokio::time::Instant,
    },
    Answered(OfferAnswer),
    /// The job failed and `rg` could not be spawned.
    SpawnFailed(GrepSearchOutput),
}

/// An offered job's hits, rendered as `rg --heading` bytes.
pub(crate) enum OfferAnswer {
    /// A complete search, or a truncated one that renders past the card's limits.
    Served(Vec<u8>),
    /// What the job found before the deadline.
    TimedOut(Vec<u8>),
}

impl GrepSource {
    /// Run an offered job until the request deadline, and spawn `rg` when it
    /// fails. A fallback `rg` gets the rest of the request deadline, not a fresh
    /// timeout. Records which path answered on `span`.
    pub(crate) async fn resolve(
        self,
        config: &GrepFormatConfig,
        span: &tracing::Span,
    ) -> ResolvedSource {
        let (job, request, rg_command) = match self {
            GrepSource::Rg(rg) => {
                span.record("answered_by", "rg");
                return ResolvedSource::Rg {
                    rg,
                    deadline: tokio::time::Instant::now() + grep_timeout(),
                };
            }
            GrepSource::Offered {
                job,
                request,
                rg_command,
            } => (job, request, rg_command),
        };
        let answer = match tokio::time::timeout_at(request.deadline, job).await {
            Err(_elapsed) => Ok(OfferAnswer::TimedOut(Vec::new())),
            Ok(Err(failed)) => Err(failed.label),
            Ok(Ok(search)) => match unsearched_path(&search, &request) {
                Some(label) => Err(label),
                None => answer_from(&search, &request, config),
            },
        };
        match answer {
            Ok(answer) => {
                span.record("answered_by", "served");
                let is_timed_out = matches!(answer, OfferAnswer::TimedOut(_));
                span.record("timed_out", is_timed_out);
                if is_timed_out {
                    span.record("grep_reason", "timeout");
                }
                ResolvedSource::Answered(answer)
            }
            Err(label) => {
                span.record("answered_by", "served_failed_then_rg");
                span.record("served_failure", label);
                match RgRunner::spawn(rg_command) {
                    Ok(rg) => ResolvedSource::Rg {
                        rg,
                        deadline: request.deadline,
                    },
                    Err(error) => ResolvedSource::SpawnFailed(spawn_failure_output(&error)),
                }
            }
        }
    }
}

/// The rendered answer for a served search, or why `rg` must answer instead.
fn answer_from(
    search: &ContentSearch,
    request: &ContentSearchRequest,
    config: &GrepFormatConfig,
) -> Result<OfferAnswer, &'static str> {
    match (render_rg_heading(search, request), search.outcome) {
        (None, _) => Err("cut_line_unrenderable"),
        (Some(rendered), ContentSearchOutcome::Complete) => Ok(OfferAnswer::Served(rendered)),
        (Some(rendered), ContentSearchOutcome::TimedOut) => Ok(OfferAnswer::TimedOut(rendered)),
        // The card would show every rendered line, so it could not say
        // "at least"; only `rg` can tell how much more there is.
        (Some(rendered), ContentSearchOutcome::Truncated)
            if rendered.len() <= MAX_STDOUT_BYTES
                && rendered.iter().filter(|&&b| b == b'\n').count()
                    <= config.effective_head_limit =>
        {
            Err("truncation_not_shown")
        }
        (Some(rendered), ContentSearchOutcome::Truncated) => Ok(OfferAnswer::Served(rendered)),
    }
}

impl OfferAnswer {
    /// Push the rendered bytes through `output` and build the card as for `rg`
    /// output. Returns the streamed deltas (none unless `output` streams) and the
    /// terminal card.
    pub(crate) fn into_card(
        self,
        mut output: CappedOutput<'_>,
        config: &GrepFormatConfig,
    ) -> (Vec<xai_tool_runtime::ToolProgress>, GrepSearchOutput) {
        let rendered = match &self {
            OfferAnswer::Served(rendered) | OfferAnswer::TimedOut(rendered) => rendered,
        };
        let accepted = output.accept(rendered);
        let mut deltas = accepted.deltas;
        deltas.extend(output.finish());
        let card = match self {
            OfferAnswer::Served(_) => {
                // No output is how `rg` reports no matches: exit 1, empty stdout.
                let exit_code = if output.is_empty() { 1 } else { 0 };
                let is_truncated = matches!(accepted.fill, Fill::Overflowed);
                finalize_grep(
                    output.into_bytes(),
                    is_truncated,
                    Vec::new(),
                    exit_code,
                    config,
                )
            }
            OfferAnswer::TimedOut(_) => {
                grep_partial_timeout_output(output.into_bytes(), grep_timeout().as_secs(), config)
            }
        };
        (deltas, card)
    }
}

/// Why `search` names a file `rg` would not have searched, if it does: one
/// outside [`ContentSearchRequest::root`], or one that the request's globs
/// exclude, including the `!<glob>` read-deny excludes. `rg` applies the globs
/// with `process_cwd` as their root to every walked file and directory, but not
/// to a root given as a file.
fn unsearched_path(search: &ContentSearch, request: &ContentSearchRequest) -> Option<&'static str> {
    let mut builder = OverrideBuilder::new(&request.process_cwd);
    for glob in &request.globs {
        if builder.add(glob).is_err() {
            return Some("glob_unparsed");
        }
    }
    let Ok(overrides) = builder.build() else {
        return Some("glob_unparsed");
    };
    for file in &search.files {
        if file.path == request.root {
            continue;
        }
        let Ok(below_root) = file.path.strip_prefix(&request.root) else {
            return Some("path_outside_root");
        };
        if below_root
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Some("path_outside_root");
        }
        if overrides.matched(&file.path, false).is_ignore() {
            return Some("path_excluded_by_glob");
        }
        let excluded_dir = below_root
            .ancestors()
            .skip(1)
            .filter(|dir| !dir.as_os_str().is_empty())
            .any(|dir| overrides.matched(request.root.join(dir), true).is_ignore());
        if excluded_dir {
            return Some("path_excluded_by_glob");
        }
    }
    None
}

/// Whether `rg` loads a user config file, given `RIPGREP_CONFIG_PATH`. `rg` reads
/// no config when the variable is unset or empty.
fn rg_reads_user_config(config_path: Option<&OsStr>) -> bool {
    config_path.is_some_and(|path| !path.is_empty())
}

/// The request describing the `rg` call `args` builds. `None` when no backend
/// could reproduce `rg`'s answer: the working directory `rg` would inherit is
/// unknown, a user config (`RIPGREP_CONFIG_PATH`) may change its flags, or the
/// count mode is multiline, where `rg -c` counts matches the hits do not delimit.
pub(crate) fn content_search_request(
    args: &RgArgs<'_>,
    root_kind: RootKind,
    effective_head_limit: usize,
) -> Option<ContentSearchRequest> {
    if rg_reads_user_config(std::env::var_os("RIPGREP_CONFIG_PATH").as_deref()) {
        tracing::debug!("RIPGREP_CONFIG_PATH is set; grep runs rg without an offer");
        return None;
    }
    let input = args.input;
    let mode = match args.output_mode {
        OutputMode::FilesWithMatches => ContentSearchMode::FilesWithMatches,
        OutputMode::Count => ContentSearchMode::Count,
        OutputMode::Content => ContentSearchMode::Content,
    };
    if mode == ContentSearchMode::Count && input.multiline {
        return None;
    }
    let process_cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            tracing::debug!(%error, "process cwd unknown; grep runs rg without an offer");
            return None;
        }
    };
    // `rg` takes `-C` first, then `-B` and `-A` override it; the tool passes
    // only positive counts.
    let context_lines = |lines: Option<usize>| {
        lines
            .filter(|&n| n > 0)
            .or(input.context.filter(|&n| n > 0))
            .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    Some(ContentSearchRequest {
        root: PathBuf::from(args.root.to_string_lossy().into_owned()),
        root_kind,
        pattern: input.pattern.clone(),
        case: if input.case_insensitive {
            CaseSensitivity::Insensitive
        } else {
            CaseSensitivity::Sensitive
        },
        globs: args.globs.to_vec(),
        file_type: input
            .r#type
            .clone()
            .filter(|file_type| !file_type.is_empty()),
        multiline: if input.multiline {
            MultilineMode::DotAll
        } else {
            MultilineMode::Off
        },
        context_before: context_lines(input.before_context),
        context_after: context_lines(input.after_context),
        mode,
        max_file_bytes: MAX_FILE_BYTES,
        max_columns: MAX_COLUMNS,
        result_budget: u32::try_from(effective_head_limit.saturating_add(1)).unwrap_or(u32::MAX),
        process_cwd,
        deadline: tokio::time::Instant::now() + grep_timeout(),
    })
}

#[cfg(test)]
#[path = "offer_tests.rs"]
mod tests;
