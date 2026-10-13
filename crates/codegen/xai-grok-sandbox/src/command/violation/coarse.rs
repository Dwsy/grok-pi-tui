//! The coarse channel: exit status plus stderr keywords — the only decoder, so every target it
//! names is a guess the card asks the user to confirm. A path is a token that starts with `/`,
//! `./` or `../`, never a bare name, joined to the command's cwd. A quoted span is one token with
//! its spaces; an unquoted one is extended across a space while the result is a directory on disk.
//! A marker line with no such token is [`Blocked::Unknown`], which never
//! produces a card.
//!
//! The Seatbelt-specific shapes live in [`macos`]; the shared errno texts are here. The battery
//! is string matching (plus one `is_dir` probe for a spaced path), so its fixtures — the real
//! captures in `tests/fixtures/violations/macos/` — are checked on every host.

pub mod macos;

use std::path::{Component, Path, PathBuf};

use crate::command::canonical::{fold_dots, is_within};
use crate::command::grants::ip_literal;
use crate::command::policy::SandboxPolicy;
use crate::command::violation::{Blocked, Capability};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Marker {
    /// Only a write can produce it: EROFS, or a Seatbelt `file-write-*` report.
    Write,
    /// Only a read can produce it: a Seatbelt `file-read-*` report.
    Read,
    /// EACCES / EPERM: read or write, decided by the verb or the policy.
    Denied,
}

/// Markers every OS shares: the errno texts and their symbolic names.
const SHARED_MARKERS: &[(&str, Marker)] = &[
    ("failed to write file", Marker::Write),
    ("operation not permitted", Marker::Denied),
    ("permission denied", Marker::Denied),
    ("eacces", Marker::Denied),
    ("eperm", Marker::Denied),
];

/// Verbs that name a write when the marker alone is ambiguous.
const WRITE_HINTS: &[&str] = &[
    "cannot touch",
    "cannot create",
    "cannot remove",
    "cannot mkdir",
    "cannot move",
    "cannot rename",
    "cannot unlink",
    "cannot change",
    "for writing",
    "could not write",
    "unable to create",
    "read-only",
    "mkdir",
    "unlink",
    "rename",
    "rmdir",
    "chmod",
    "chown",
    "truncate",
    "writefile",
    "syscall write",
];

/// Programs whose denial on a path is a read; the path itself is usually not writable either, so
/// the policy cannot tell the two apart.
const READ_PROGRAMS: &[&str] = &[
    "cat", "head", "tail", "less", "more", "grep", "rg", "ls", "stat", "find", "wc", "file",
    "diff", "sort", "readlink", "realpath",
];

const READ_HINTS: &[&str] = &["cannot open", "cannot access", "cannot read", "for reading"];

/// Automount roots: a lookup of `/net/<host>`, `/Network/…` or `/Volumes/<name>` can mount a
/// remote share and stall until it answers.
const REMOTE_MOUNT_ROOTS: &[&str] = &["net", "Network", "Volumes"];

#[cfg(test)]
thread_local! {
    /// Directories [`extend_across_space`] asked the filesystem about on this thread.
    pub(super) static DIR_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether the stderr looks like a sandbox denial at all. A status code of 0, 126 or 127 never
/// does; the caller has already rejected those. Lines that are never a sandbox denial
/// ([`is_never_sandbox`]) do not count.
pub fn is_likely_denial(stderr: &str) -> bool {
    stderr
        .lines()
        .filter(|line| !is_never_sandbox(line))
        .any(|line| first_marker(&line.to_ascii_lowercase()).is_some())
}

/// Denial texts that come from somewhere other than this sandbox, whatever the marker says:
/// a remote refusing a key or a password (`git@github.com: Permission
/// denied (publickey)`, `Permission denied, please try again`), and a local socket the OS
/// refused (`dial unix /var/run/docker.sock: connect: permission denied` — a socket the user
/// cannot reason about on a card, and one the coarse channel cannot attribute).
pub fn is_never_sandbox(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if lower.contains("permission denied, please try again") {
        return true;
    }
    if lower.contains("connect: permission denied") || lower.contains("dial unix ") {
        return true;
    }
    if let Some(at) = lower.find(": permission denied") {
        let before = lower.get(..at).unwrap_or_default();
        let subject = before.rsplit([' ', '\t']).next().unwrap_or_default();
        if subject.contains('@') && !subject.contains('/') {
            return true;
        }
    }
    false
}

/// The coarse decode. The Seatbelt setuid refusal is recognised first (it names no path), then
/// network messages (they carry no filesystem path), then the first marker line with a path
/// outside a `__pycache__` ([`is_pycache_noise`]), a capability by program name, then `Unknown`
/// — or `None` when byte-code paths were the only ones named. The caller screens the exit status
/// (0, 126 and 127 never reach here); the battery reads only the text.
pub fn decode_coarse(stderr: &str, cwd: &Path, policy: &SandboxPolicy) -> Option<Blocked> {
    let network = network_failure(stderr);
    if !is_likely_denial(stderr) && network.is_none() {
        return None;
    }
    if setuid_exec_refused(stderr) {
        return Some(Blocked::Capability {
            what: Capability::SetUid,
        });
    }
    if let Some((host, port)) = network {
        return Some(Blocked::Net { host, port });
    }
    let (mut byte_code_noise, mut pathless_marker) = (false, false);
    for line in stderr.lines().filter(|line| !is_never_sandbox(line)) {
        let lower = line.to_ascii_lowercase();
        let Some((marker_at, marker)) = first_marker(&lower) else {
            continue;
        };
        if let Some(what) = capability_by_program(&lower) {
            return Some(Blocked::Capability { what });
        }
        let remote = |path: &Path| under_remote_mount_root(path, &policy.write_roots);
        let path = match path_token(line, marker_at, cwd, &policy.write_roots) {
            Some(LinePath::One(token)) => Some(lexical_join(cwd, &token)).filter(|p| !remote(p)),
            // The text does not say which side was refused: the source, unless only the
            // destination is protected (a staged tree moved in as `.git`)
            Some(LinePath::Rename { from, to }) => {
                let (from, to) = (lexical_join(cwd, &from), lexical_join(cwd, &to));
                if remote(&from) || remote(&to) {
                    None
                } else if !policy.is_protected(&from) && policy.is_protected(&to) {
                    Some(to)
                } else {
                    Some(from)
                }
            }
            None => None,
        };
        let Some(path) = path else {
            pathless_marker = true;
            continue;
        };
        if is_pycache_noise(&path) {
            byte_code_noise = true;
            continue;
        }
        let is_write = match marker {
            Marker::Write => true,
            Marker::Read => false,
            Marker::Denied => {
                if READ_HINTS.iter().any(|hint| lower.contains(hint))
                    || read_program(&lower).is_some()
                {
                    false
                } else if WRITE_HINTS.iter().any(|hint| lower.contains(hint)) {
                    true
                } else {
                    // Neither verb: the policy says which of the two it would have refused (a read
                    // deny refuses both, so it reads as the read). When it refuses neither the
                    // write is reported and `decode` drops it as noise.
                    let write_refused = !policy
                        .would_allow(&Blocked::FsWrite { path: path.clone() })
                        && !policy.is_read_floor(&path);
                    let read_refused = !policy.would_allow(&Blocked::FsRead { path: path.clone() });
                    write_refused || !read_refused
                }
            }
        };
        return Some(if is_write {
            Blocked::FsWrite { path }
        } else {
            Blocked::FsRead { path }
        });
    }
    if byte_code_noise && !pathless_marker {
        return None;
    }
    Some(Blocked::Unknown {
        stderr_snippet: stderr_tail(stderr, crate::command::violation::STDERR_SNIPPET_MAX_BYTES),
    })
}

/// `sandbox-exec: execvp() of '/usr/bin/sudo' failed: Operation not permitted` (exit 71, no
/// `Sandbox:` line): Seatbelt refused a setuid exec before any rule ran.
fn setuid_exec_refused(stderr: &str) -> bool {
    stderr.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("sandbox-exec: execvp() of")
            && lower.contains("failed: operation not permitted")
    })
}

/// The earliest marker on `lower`: a shared errno text or the Seatbelt `deny(1) <operation>`
/// report — never a bare `sandbox` word.
fn first_marker(lower: &str) -> Option<(usize, Marker)> {
    SHARED_MARKERS
        .iter()
        .filter_map(|(text, marker)| lower.find(text).map(|at| (at, *marker)))
        .chain(macos::deny_report(lower))
        .min_by_key(|(at, _)| *at)
}

/// `mount: …`, `lldb: …`, `sudo: …` naming themselves: a capability the sandbox refused.
fn capability_by_program(lower: &str) -> Option<Capability> {
    match program_of(lower)? {
        "mount" | "umount" => Some(Capability::Mount),
        "lldb" | "gdb" | "dtrace" | "dtruss" => Some(Capability::Ptrace),
        "su" | "sudo" | "newgrp" => Some(Capability::SetUid),
        _ => None,
    }
}

fn read_program(lower: &str) -> Option<&'static str> {
    let program = program_of(lower)?;
    READ_PROGRAMS.iter().copied().find(|p| *p == program)
}

/// The program a line names itself by: the text before its first `:`, less any directory.
fn program_of(lower: &str) -> Option<&str> {
    lower.split(':').next()?.trim().rsplit('/').next()
}

/// A line's path: one token, or both sides of `mv`'s `rename A to B`.
enum LinePath {
    One(String),
    Rename { from: String, to: String },
}

/// The path token nearest before the marker, else the first after it — except `mv`'s `rename A
/// to B`, which yields both: the kernel refused unlinking A (the macOS capture) or creating B.
/// A token inside a URL (`https://registry.npmjs.org/pkg` splits into `https` and
/// `//registry.npmjs.org/pkg`) is never a path.
fn path_token(
    line: &str,
    marker_at: usize,
    cwd: &Path,
    write_roots: &[PathBuf],
) -> Option<LinePath> {
    let urls = url_spans(line);
    let mut before: Vec<String> = Vec::new();
    let mut after: Option<String> = None;
    for token in path_tokens(line, cwd, write_roots) {
        if urls.iter().any(|span| span.contains(&token.start)) {
            continue;
        }
        if token.start < marker_at {
            before.push(token.path);
        } else if after.is_none() {
            after = Some(token.path);
        }
    }
    let prefix = line.get(..marker_at).unwrap_or_default();
    if let Some((from, to)) = rename_sides(prefix) {
        return Some(LinePath::Rename { from, to });
    }
    before.pop().or(after).map(LinePath::One)
}

/// The two sides of `mv: rename A to B: `, as printed: relative names too (`mv stage .git`) and
/// spaces kept. A destination that itself contains ` to ` is split wrongly, a limit of the text.
fn rename_sides(prefix: &str) -> Option<(String, String)> {
    let body = prefix.trim_end().strip_suffix(':')?;
    let (_, sides) = body.split_once("rename ")?;
    let (from, to) = sides.rsplit_once(" to ")?;
    let unquote = |side: &str| {
        side.trim()
            .trim_matches(|c| matches!(c, '\'' | '"' | '`'))
            .to_owned()
    };
    let (from, to) = (unquote(from), unquote(to));
    (!from.is_empty() && !to.is_empty()).then_some((from, to))
}

/// Byte ranges of the whitespace-delimited words that carry a URL scheme (`scheme://…`).
fn url_spans(line: &str) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    for (index, c) in line
        .char_indices()
        .chain(std::iter::once((line.len(), ' ')))
    {
        match (c.is_whitespace(), start) {
            (true, Some(begin)) => {
                if line
                    .get(begin..index)
                    .is_some_and(|word| word.contains("://"))
                {
                    spans.push(begin..index);
                }
                start = None;
            }
            (false, None) => start = Some(index),
            (true, None) | (false, Some(_)) => {}
        }
    }
    spans
}

/// A path token read from a message line: where it starts and the path text.
struct PathToken {
    start: usize,
    path: String,
}

/// The path tokens of `line` in order. A quoted span (`'…'`, `"…"`, `` `…` ``, `‘…’`, `“…”`)
/// is one token, spaces and all — node's `open '/a b/c'`, python's `'/a b/c'`. An unquoted token
/// runs to whitespace or `:` and is then extended across a single space while the extension is a
/// directory on disk (`~/Library/Application Support/MyTool/state.json` after bash's unquoted
/// `<path>: Operation not permitted`). Only `/`, `./`, `../` and `.dir/` starts count, so
/// `node:fs:2426`, `[eval]:1:15` and `link/` never match.
fn path_tokens(line: &str, cwd: &Path, write_roots: &[PathBuf]) -> Vec<PathToken> {
    let mut words = words_with_offsets(line).into_iter().peekable();
    let mut out = Vec::new();
    while let Some(word) = words.next() {
        let Some(mut path) = as_path_token(word.text, word.quoted) else {
            continue;
        };
        let mut end = word.end;
        while !word.quoted
            && let Some(next) = words.peek()
            && let Some(extended) = extend_across_space(
                &path,
                line.get(end..next.start).unwrap_or_default(),
                next.text,
                cwd,
                write_roots,
            )
        {
            path = extended;
            end = next.end;
            words.next();
        }
        out.push(PathToken {
            start: word.start,
            path,
        });
    }
    out
}

struct Word<'a> {
    start: usize,
    end: usize,
    text: &'a str,
    quoted: bool,
}

/// `path` + the space + `next`, when `next` is not itself a path token, the gap is one space and
/// the directory the spaced component names exists — `…/Application` + `Support/MyTool/x` is
/// taken when `…/Application Support` is a directory (through a symlink too: the tool's spelling
/// may pass through one). Asked at the command's `cwd`, and never under an automount root
/// ([`under_remote_mount_root`]).
fn extend_across_space(
    path: &str,
    gap: &str,
    next: &str,
    cwd: &Path,
    write_roots: &[PathBuf],
) -> Option<String> {
    if gap != " " || as_path_token(next, false).is_some() {
        return None;
    }
    let component = next.split('/').next().unwrap_or(next);
    let spaced = lexical_join(cwd, &format!("{path} {component}"));
    if under_remote_mount_root(&spaced, write_roots) {
        return None;
    }
    #[cfg(test)]
    DIR_PROBES.set(DIR_PROBES.get() + 1);
    spaced.is_dir().then(|| format!("{path} {next}"))
}

/// Whether `path` lies beneath an automount root ([`REMOTE_MOUNT_ROOTS`], any ASCII case, as
/// APFS finds them) and in no write root: a path read from a command's output is never looked up
/// there. Lexical, so a symlink into one is still followed.
fn under_remote_mount_root(path: &Path, write_roots: &[PathBuf]) -> bool {
    let mut components = path.components();
    let beneath = components.next() == Some(Component::RootDir)
        && components.next().is_some_and(|first| {
            REMOTE_MOUNT_ROOTS
                .iter()
                .any(|root| first.as_os_str().eq_ignore_ascii_case(root))
        })
        && components.next().is_some();
    beneath && !write_roots.iter().any(|root| is_within(path, root))
}

/// Words with their byte offsets: a quoted span is one word (quotes stripped, `quoted` set);
/// otherwise a run up to whitespace or `:`.
fn words_with_offsets(line: &str) -> Vec<Word<'_>> {
    let mut out = Vec::new();
    let mut rest = line.char_indices().peekable();
    while let Some((start, c)) = rest.next() {
        if c.is_whitespace() || c == ':' {
            continue;
        }
        if let Some(close) = closing_quote(c)
            && let Some(offset) = line.get(start + c.len_utf8()..).and_then(|s| s.find(close))
        {
            let text_start = start + c.len_utf8();
            let text_end = text_start + offset;
            let end = text_end + close.len_utf8();
            out.push(Word {
                start,
                end,
                text: line.get(text_start..text_end).unwrap_or_default(),
                quoted: true,
            });
            while rest.peek().is_some_and(|(index, _)| *index < end) {
                rest.next();
            }
            continue;
        }
        let mut end = line.len();
        while let Some((index, next)) = rest.peek().copied() {
            if next.is_whitespace() || next == ':' {
                end = index;
                break;
            }
            rest.next();
        }
        out.push(Word {
            start,
            end,
            text: line.get(start..end).unwrap_or_default(),
            quoted: false,
        });
    }
    out
}

fn closing_quote(open: char) -> Option<char> {
    match open {
        '\'' | '"' | '`' => Some(open),
        '‘' => Some('’'),
        '“' => Some('”'),
        _ => None,
    }
}

/// The path a word names, or `None` for anything that does not start like one. An unquoted word
/// loses the punctuation a sentence wraps it in; a quoted one is taken as is.
pub(super) fn as_path_token(word: &str, quoted: bool) -> Option<String> {
    let core = if quoted {
        word
    } else {
        let unquoted = word.trim_matches(|c: char| {
            matches!(
                c,
                '\'' | '"' | '`' | '‘' | '’' | '“' | '”' | ',' | ';' | '(' | ')' | '[' | ']'
            )
        });
        // A sentence-ending period is dropped; the dots of `.`, `..` and `dir/..` are path syntax
        if unquoted.ends_with('.') && !unquoted.ends_with("..") && !unquoted.ends_with("/.") {
            unquoted.trim_end_matches('.')
        } else {
            unquoted
        }
    };
    let is_path = core.starts_with('/')
        || core.starts_with("./")
        || core.starts_with("../")
        || is_dot_dir_relative(core);
    is_path.then(|| core.to_owned())
}

/// `.grok/config.toml`, `.git/config`: a cwd-relative path into a dot directory, which the shell
/// prints as typed (`sh: cannot create .grok/config.toml: Read-only file system`). A bare `.name`
/// with no `/` is not taken (it could be a file name, an option or `...`), nor is `..name`.
fn is_dot_dir_relative(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('.') else {
        return false;
    };
    let starts_like_a_name = rest
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-');
    starts_like_a_name && rest.contains('/')
}

/// `cwd`-relative join with `.` and `..` folded lexically; the target may not exist, so no
/// canonicalisation.
pub fn lexical_join(cwd: &Path, token: &str) -> PathBuf {
    fold_dots(&cwd.join(token))
}

/// The directory a grant for `path` proposes: `path` itself when it is an existing directory,
/// otherwise the closest existing ancestor (a file is never proposed, the card shows a tree).
/// Through symlinks on purpose: a grant applies to, and meets the floor at, its canonical path.
pub fn nearest_existing_dir(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }
    let mut cursor = path.parent();
    while let Some(dir) = cursor {
        if dir.is_dir() {
            return dir.to_path_buf();
        }
        cursor = dir.parent();
    }
    path.parent().unwrap_or(path).to_path_buf()
}

/// Whether `path` sits under a `__pycache__` directory: Python writes byte-code beside every
/// module it imports and runs on without it, so a denied one is noise [`decode_coarse`] reads
/// past (never a card) — the command's real target is elsewhere.
pub fn is_pycache_noise(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "__pycache__")
}

/// curl / wget / getaddrinfo shapes for a blocked connection or resolution, bash's `/dev/tcp`
/// refusal and the Seatbelt `network-outbound` report. The loopback proxy itself refusing is not
/// a sandbox violation and returns `None`. The host is `None` when the message names none — a
/// resolver failure with no host, `connect: Operation not permitted`, the masked `remote:*:<port>`
/// (the `curl`, `/dev/tcp` and `getaddrinfo` captures).
fn network_failure(stderr: &str) -> Option<(Option<String>, Option<u16>)> {
    for line in stderr.lines().filter(|line| !is_never_sandbox(line)) {
        let named = connect_failure(line)
            .or_else(|| after_phrase(line, "Could not resolve host:").map(|h| (h, None)))
            .or_else(|| after_phrase(line, "Could not resolve proxy:").map(|h| (h, None)))
            .or_else(|| after_phrase(line, "getaddrinfo ENOTFOUND").map(|h| (h, None)))
            .or_else(|| after_phrase(line, "getaddrinfo EAI_AGAIN").map(|h| (h, None)))
            .or_else(|| after_phrase(line, "unable to resolve host address").map(|h| (h, None)));
        if let Some((host, port)) = named {
            if is_loopback(&host) {
                return None;
            }
            return Some((Some(host), port));
        }
        if let Some(at) = resolver_phrase_at(line) {
            return Some((host_before(line, at), None));
        }
        if bare_connect_refused(line) {
            return Some((None, None));
        }
        if let Some(port) = macos::network_report(line) {
            return Some((None, port));
        }
    }
    None
}

/// Where the libc resolver's phrase starts on `line`, if it is one: `nodename nor servname
/// provided, or not known` (macOS; bash puts the host in the segment before it, python's
/// `socket.gaierror: [Errno 8]` names none), plus the glibc spellings tools ship in their own
/// resolver text.
fn resolver_phrase_at(line: &str) -> Option<usize> {
    let lower = line.to_ascii_lowercase();
    [
        "nodename nor servname provided",
        "temporary failure in name resolution",
        "name or service not known",
    ]
    .into_iter()
    .find_map(|phrase| lower.find(phrase))
}

/// `/bin/bash: connect: Operation not permitted` — bash's `/dev/tcp/<host>/<port>` redirection
/// refused by the kernel; the host and port are in the command line, never in the message.
fn bare_connect_refused(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let Some(at) = first_marker(&lower).map(|(at, _)| at) else {
        return false;
    };
    let before = lower
        .get(..at)
        .unwrap_or_default()
        .trim_end_matches([':', ' ']);
    before
        .rsplit(": ")
        .next()
        .is_some_and(|segment| segment.trim() == "connect")
}

/// The `: `-separated segment right before byte `at`, when it looks like a host name (has a dot,
/// no path or space in it).
fn host_before(line: &str, at: usize) -> Option<String> {
    let before = line.get(..at)?.trim_end_matches([':', ' ']);
    let segment = before.rsplit(": ").next()?.trim();
    let is_host = !segment.is_empty()
        && segment.contains('.')
        && !segment.contains(['/', ' ', '\'', '"', '[']);
    is_host.then(|| segment.to_owned())
}

/// `Failed to connect to <host> port <n>` (curl), `connect to host <host> port <n>` (ssh) and
/// `connect to <host> port <n>` variants. A misconfigured git that ignores the proxy env (a
/// `http.proxy` in `~/.gitconfig`) fails closed under `Enforce` with one of these. The word
/// after `connect to` is a host only when a `port` follows or it has a dot or a colon in it, so
/// prose (`connect to the Docker daemon socket`) is not one.
fn connect_failure(line: &str) -> Option<(String, Option<u16>)> {
    let idx = line.find("onnect to ")?;
    let rest = line.get(idx + "onnect to ".len()..)?;
    let mut words = rest.split_whitespace();
    let mut host = words.next()?.trim_matches(['\'', '"', ',']);
    if host == "host" {
        host = words.next()?.trim_matches(['\'', '"', ',']);
    }
    let port = match (words.next(), words.next()) {
        (Some("port"), Some(port)) => port.trim_end_matches([':', ',']).parse().ok(),
        _ => None,
    };
    if host.is_empty() || host.starts_with('/') {
        return None;
    }
    if port.is_none() && !host.contains(['.', ':']) {
        return None;
    }
    Some((host.to_owned(), port))
}

fn after_phrase(line: &str, phrase: &str) -> Option<String> {
    let idx = line.find(phrase)?;
    let rest = line.get(idx + phrase.len()..)?;
    let host = rest
        .split_whitespace()
        .next()?
        .trim_matches(['\'', '"', ',', '(', ')', ';']);
    (!host.is_empty()).then(|| host.to_owned())
}

/// `localhost`, or an address in `127.0.0.0/8`, `::1` or the IPv4-mapped form of the first,
/// parsed: `127.example.com` is a remote host.
fn is_loopback(host: &str) -> bool {
    host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
        || ip_literal(host).is_some_and(|address| address.to_canonical().is_loopback())
}

/// The last `max_bytes` of `stderr` on a char boundary, trimmed.
pub fn stderr_tail(stderr: &str, max_bytes: usize) -> String {
    let trimmed = stderr.trim();
    if trimmed.len() <= max_bytes {
        return trimmed.to_owned();
    }
    let mut start = trimmed.len() - max_bytes;
    while !trimmed.is_char_boundary(start) {
        start += 1;
    }
    trimmed.get(start..).unwrap_or_default().to_owned()
}

#[cfg(test)]
#[path = "coarse_tests.rs"]
mod tests;
