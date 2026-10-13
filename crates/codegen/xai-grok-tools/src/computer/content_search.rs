//! A content search a file system backend can answer instead of `rg`.
//!
//! The grep tool describes each call as a [`ContentSearchRequest`] and asks the
//! session's file system through
//! [`AsyncFileSystem::offer_content_search`](crate::computer::types::AsyncFileSystem::offer_content_search).
//! A backend that declines returns `None` without doing any I/O, and the tool
//! runs `rg` as usual. An offered [`ContentSearchJob`] runs first: the tool
//! renders its hits as the bytes `rg` prints and passes them through the same
//! caps, streaming and result card as `rg` output. A failed job makes the tool
//! run `rg` instead.
//!
//! Contract for a served result: exactly the lines `rg` 15 reports for the same
//! request, with files in `rg --sort path` order. When `rg` would print anything
//! the hits cannot express (an error for an unreadable file, or the binary-file
//! notice after matches found before a NUL byte), or would exit with an error,
//! the job must fail instead. A backend may leave out a file that holds a NUL
//! byte only when it has no match before its first NUL, which is when `rg`
//! prints nothing for it.
//!
//! The tool runs `rg` without `--sort`, so `rg` orders files as its parallel walk
//! finds them. A served result is therefore one of the orders `rg` can print,
//! and when the head limit cuts a result, the served lines and an `rg` answer to
//! the same call can be different subsets, as two `rg` runs can.
//!
//! [`ContentSearchRequest::globs`] carries the read-deny excludes, so honoring
//! them is a security obligation of the backend: it must never read a file they
//! exclude. The tool also drops a served result that names a file outside the
//! root or excluded by a glob, and runs `rg` instead.

use std::path::PathBuf;

use futures::future::BoxFuture;

/// Longest [`HitLine::text`] a backend needs to report. The tool renders at
/// most about 5 MB of `rg` output from a result and ignores the rest, so a
/// larger result costs only the backend's own memory.
pub const MAX_HIT_LINE_BYTES: usize = 4000;

/// An offered search. Does nothing until polled, and is dropped unfinished at
/// the request deadline.
pub type ContentSearchJob = BoxFuture<'static, Result<ContentSearch, ContentSearchFailed>>;

/// One grep call, described by the `rg` arguments the tool would pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSearchRequest {
    /// The search root exactly as passed to `rg`. Printed paths start with it.
    pub root: PathBuf,
    pub root_kind: RootKind,
    /// The `rg -e` pattern (Rust regex syntax).
    pub pattern: String,
    pub case: CaseSensitivity,
    /// `rg --glob` values in the tool's order: the caller's glob, then `!<glob>`
    /// for each deny-read glob. The last matching glob wins, and a glob with a
    /// `/` anchors at [`ContentSearchRequest::process_cwd`].
    pub globs: Vec<String>,
    /// The `rg --type` name.
    pub file_type: Option<String>,
    pub multiline: MultilineMode,
    /// Context lines before each match, after `rg` applied `-C`, then `-B`.
    pub context_before: u32,
    /// Context lines after each match, after `rg` applied `-C`, then `-A`.
    pub context_after: u32,
    pub mode: ContentSearchMode,
    /// Files larger than this are skipped (`rg --max-filesize`).
    pub max_file_bytes: u64,
    /// `rg --max-columns` with `--max-columns-preview`, applied when the hits are
    /// rendered.
    pub max_columns: u32,
    /// The tool shows fewer output lines than this. A backend may stop after this
    /// many match lines (files, in the files and count modes) and report
    /// [`ContentSearchOutcome::Truncated`]. Advisory: the tool caps what it
    /// renders either way.
    pub result_budget: u32,
    /// The working directory `rg` inherits (it runs with none set).
    pub process_cwd: PathBuf,
    /// When the tool stops waiting and reports a timeout.
    pub deadline: tokio::time::Instant,
}

/// What the search root was when the tool checked it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RootKind {
    Directory,
    File,
    /// Neither, or the check failed.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CaseSensitivity {
    Sensitive,
    /// `rg --ignore-case`.
    Insensitive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MultilineMode {
    Off,
    /// `rg -U --multiline-dotall`: matches may span lines and `.` matches `\n`.
    DotAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContentSearchMode {
    /// Matching lines with their context.
    Content,
    /// `rg -l`.
    FilesWithMatches,
    /// `rg -c`. Never combined with [`MultilineMode::DotAll`]: the tool runs
    /// `rg` for a multiline count.
    Count,
}

/// A served search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSearch {
    /// Files with hits, in `rg --sort path` order (component-wise
    /// [`Path`](std::path::Path) ordering, so `dir/x` sorts before `dir.txt`).
    pub files: Vec<FileHits>,
    pub outcome: ContentSearchOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentSearchOutcome {
    /// Every match was found.
    Complete,
    /// A cap stopped the search and more results exist.
    Truncated,
    /// The deadline passed. The files hold what was found before it.
    TimedOut,
}

/// The hits in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHits {
    /// Exactly as `rg` prints it: [`ContentSearchRequest::root`] joined with the
    /// path below it.
    pub path: PathBuf,
    /// Match and context lines by ascending line number. May be empty in the
    /// files mode. The count mode counts the match lines.
    pub lines: Vec<HitLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitLine {
    /// 1-based.
    pub line_no: u64,
    pub kind: HitKind,
    /// The line's raw bytes without its `\n` (a `\r` stays), at most
    /// [`MAX_HIT_LINE_BYTES`]. `rg` prints the first `max_columns` grapheme
    /// clusters of a long line, so a cut text must hold more clusters than that
    /// or the tool runs `rg` instead.
    pub text: Vec<u8>,
    /// The line was longer than `text`.
    pub is_cut: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    Match,
    Context,
}

/// The job could not answer. The tool runs `rg` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("content search failed: {label}")]
pub struct ContentSearchFailed {
    /// Chosen by the backend, for tracing and telemetry.
    pub label: &'static str,
}
