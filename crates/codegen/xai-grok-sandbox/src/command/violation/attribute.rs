//! Which paths a command can be said to have reached for. The coarse
//! channel reads a path out of stderr; nothing ties that text to the process, so a printed
//! denial plus `exit 1` would otherwise buy a card for any directory the breadth caps allow. The
//! card proposes a grant only for a path inside the call's own candidate set: its cwd, the path
//! tokens of its argv (joined to the cwd) and their ancestors, the served root, and the bases its
//! environment named (`$PYTHONUSERBASE`, `$XDG_DATA_HOME`). A denial naming any other path is
//! `informational: unattributed`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::command::canonical::is_within;
use crate::command::violation::coarse::{as_path_token, lexical_join};

/// Bounded so a pathological argv (a generated script) cannot make the screen quadratic.
const MAX_NAMED: usize = 256;

/// The paths one command reached for. Built once per decode from the call's record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnTargets {
    /// Trees the command owns wholesale: a path below any of them is the command's.
    trees: Vec<PathBuf>,
    /// Paths the argv named: the path itself, anything below it, and its ancestors (`mkdir -p a/b`
    /// is refused on `a`; the shell creates the parent of a redirection target).
    named: Vec<PathBuf>,
}

impl OwnTargets {
    /// `argv` is the command's arguments without the program (`bash -lc '<script>'` names its
    /// paths inside the script, so every argument is scanned as text). `user_home` expands a
    /// leading `~/`; without it a `~` token names nothing.
    pub fn of<'a>(
        cwd: &Path,
        argv: impl IntoIterator<Item = &'a OsStr>,
        served_root: &Path,
        env_bases: &[PathBuf],
        user_home: Option<&Path>,
    ) -> OwnTargets {
        let mut trees = vec![cwd.to_path_buf(), served_root.to_path_buf()];
        trees.extend(env_bases.iter().cloned());
        let mut named = Vec::new();
        for arg in argv {
            let Some(text) = arg.to_str() else {
                continue;
            };
            for token in shell_path_tokens(text) {
                if named.len() >= MAX_NAMED {
                    break;
                }
                let path = match token.strip_prefix("~/") {
                    Some(rest) => match user_home {
                        Some(home) => home.join(rest),
                        None => continue,
                    },
                    None => lexical_join(cwd, &token),
                };
                named.push(path);
            }
        }
        OwnTargets { trees, named }
    }

    /// Whether a denial naming `path` is the command's own, compared as the host's volumes compare
    /// ([`is_within`]): a denial quoting `/private/tmp/x` or the on-disk case of a folder the
    /// command spelled `/tmp/x` or in another case is still the command's.
    pub fn covers(&self, path: &Path) -> bool {
        self.trees.iter().any(|tree| is_within(path, tree))
            || self
                .named
                .iter()
                .any(|named| is_within(path, named) || is_within(named, path))
    }
}

/// Shell operators and the punctuation an argument wraps a path in: a token boundary when
/// unquoted.
const SEPARATORS: &[char] = &[
    '<', '>', '|', ';', '&', '(', ')', '=', ',', '[', ']', '{', '}',
];

/// The path-like tokens of one argument as a shell would read it: a quoted span is one token
/// (spaces kept); unquoted text splits on whitespace and [`SEPARATORS`]. `~/x` is kept as
/// written for the caller to expand. Quoted text that is not itself a path is scanned again
/// (`python3 -c 'open("/etc/x", "w")'` names `/etc/x`), one level deep.
fn shell_path_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_path_tokens(text, 1, &mut out);
    out
}

fn collect_path_tokens(text: &str, depth: usize, out: &mut Vec<String>) {
    let mut rest = text;
    while !rest.is_empty() {
        let trimmed =
            rest.trim_start_matches(|c: char| c.is_whitespace() || SEPARATORS.contains(&c));
        if trimmed.is_empty() {
            return;
        }
        let (word, quoted, after) = next_word(trimmed);
        rest = after;
        if let Some(path) = path_like(word) {
            out.push(path);
        } else if quoted && depth > 0 {
            collect_path_tokens(word, depth - 1, out);
        }
    }
}

/// One word from the front of `text`: the inside of a quoted span, or a run to whitespace or a
/// separator. Returns the word, whether it was quoted, and what follows it.
fn next_word(text: &str) -> (&str, bool, &str) {
    let mut chars = text.chars();
    if let Some(open) = chars.next()
        && matches!(open, '\'' | '"')
        && let Some(close) = text
            .get(open.len_utf8()..)
            .and_then(|inner| inner.find(open))
    {
        let inner_start = open.len_utf8();
        let inner = text
            .get(inner_start..inner_start + close)
            .unwrap_or_default();
        let after = text
            .get(inner_start + close + open.len_utf8()..)
            .unwrap_or_default();
        return (inner, true, after);
    }
    let end = text
        .find(|c: char| c.is_whitespace() || SEPARATORS.contains(&c))
        .unwrap_or(text.len());
    let (word, after) = text.split_at(end);
    (word, false, after)
}

/// `/…`, `./…`, `../…`, `.dir/…` or `~/…`; never a `$VAR` or a bare name.
fn path_like(word: &str) -> Option<String> {
    if word.starts_with("~/") {
        return Some(word.to_owned());
    }
    as_path_token(word, false)
}

#[cfg(test)]
#[path = "attribute_tests.rs"]
mod tests;
