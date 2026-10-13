//! Parity of `render_rg_heading` with `rg` 15.0.0.
//!
//! Each case pairs hand-written hits with the bytes `rg` 15.0.0 printed for the
//! same search of the tree `write_tree` builds, checked in under
//! `tests/rg-15.0.0-heading/`. Output holding invalid UTF-8 is checked in with
//! each line escaped (`*.stdout.escaped`), so every golden file is valid UTF-8
//! text. Recapture them with
//! `UPDATE_RG_HEADING_GOLDENS=1 RG_BIN_PATH=<rg 15.0.0> cargo test -p xai-grok-tools -- rg_heading`.

use std::path::{Path, PathBuf};

use bstr::{BString, ByteVec};
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::computer::content_search::{
    CaseSensitivity, ContentSearchOutcome, FileHits, HitLine, MAX_HIT_LINE_BYTES, MultilineMode,
    RootKind,
};
use crate::implementations::grok_build::grep::ripgrep::rg_path;

const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";

fn tree() -> Vec<(&'static str, Vec<u8>)> {
    let line = |parts: &[&[u8]]| parts.concat();
    let context_file: String = [
        "one",
        "needle two",
        "three",
        "needle four",
        "five",
        "six",
        "seven",
        "eight",
        "needle nine",
        "ten",
        "eleven",
        "needle twelve",
    ]
    .iter()
    .map(|text| format!("{text}\n"))
    .collect();
    vec![
        ("order/B.txt", b"needle B\n".to_vec()),
        ("order/a.txt", b"needle one\nplain\nneedle two\n".to_vec()),
        ("order/dir/x", b"needle x\n".to_vec()),
        ("order/dir-y.txt", b"needle y\n".to_vec()),
        ("order/dir.txt", b"needle dot\n".to_vec()),
        ("order/none.txt", b"nothing here\n".to_vec()),
        ("ctx/c.txt", context_file.into_bytes()),
        ("ctx/d.txt", b"needle d\nafter d\n".to_vec()),
        (
            "long/a_ascii_1500.txt",
            line(&[b"needle", &[b'x'; 1494], b"\n"]),
        ),
        (
            "long/b_fit_999.txt",
            line(&[b"needle", &[b'x'; 993], b"\n"]),
        ),
        (
            "long/c_exact_1000.txt",
            line(&[b"needle", &[b'x'; 994], b"\n"]),
        ),
        (
            "long/d_e_acute_600.txt",
            line(&[b"needle ", "é".repeat(600).as_bytes(), b"\n"]),
        ),
        (
            "long/e_e_acute_1200.txt",
            line(&[b"needle ", "é".repeat(1200).as_bytes(), b"\n"]),
        ),
        (
            "invalid/f_invalid.txt",
            line(&[b"needle ", &b"\xff\xfe".repeat(700), b"\n"]),
        ),
        (
            "long/g_crlf_cut.txt",
            line(&[b"needle", &[b'x'; 993], b"\r\n"]),
        ),
        (
            "long/h_crlf_fit.txt",
            line(&[b"needle", &[b'x'; 992], b"\r\n"]),
        ),
        (
            "long/i_combining_600.txt",
            line(&[b"needle ", "e\u{301}".repeat(600).as_bytes(), b"\n"]),
        ),
        (
            "long/j_combining_1200.txt",
            line(&[b"needle ", "e\u{301}".repeat(1200).as_bytes(), b"\n"]),
        ),
        (
            "long/k_huge_5000.txt",
            line(&[b"needle", &[b'x'; 4994], b"\n"]),
        ),
        (
            "invalid/l_invalid_subpart.txt",
            line(&[b"needle ", b"\xe2\x82", &[b'a'; 1000], b"\n"]),
        ),
        (
            "long/m_zwj.txt",
            line(&[b"needle ", FAMILY.repeat(200).as_bytes(), b"\n"]),
        ),
        (
            "longctx/lc.txt",
            line(&[&[b'y'; 1200], b"\nneedle\n", &[b'z'; 1001], b"\n"]),
        ),
        ("empty/nothing.txt", b"nothing\n".to_vec()),
    ]
}

fn write_tree(dir: &Path) {
    for (path, bytes) in tree() {
        let path = dir.join("tree").join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

/// Line 1 of `tree/<path>`, as the backend reports it.
fn first_line(path: &str) -> Vec<u8> {
    let (_, bytes) = tree().into_iter().find(|(p, _)| *p == path).unwrap();
    bytes.split(|&b| b == b'\n').next().unwrap().to_vec()
}

fn matched(line_no: u64, text: &[u8]) -> HitLine {
    HitLine {
        line_no,
        kind: HitKind::Match,
        text: text.to_vec(),
        is_cut: false,
    }
}

fn context(line_no: u64, text: &[u8]) -> HitLine {
    HitLine {
        kind: HitKind::Context,
        ..matched(line_no, text)
    }
}

fn file(path: &str, lines: Vec<HitLine>) -> FileHits {
    FileHits {
        path: PathBuf::from(format!("tree/{path}")),
        lines,
    }
}

fn order_files() -> Vec<FileHits> {
    vec![
        file("order/B.txt", vec![matched(1, b"needle B")]),
        file(
            "order/a.txt",
            vec![matched(1, b"needle one"), matched(3, b"needle two")],
        ),
        file("order/dir/x", vec![matched(1, b"needle x")]),
        file("order/dir-y.txt", vec![matched(1, b"needle y")]),
        file("order/dir.txt", vec![matched(1, b"needle dot")]),
    ]
}

struct Case {
    name: &'static str,
    golden: &'static [u8],
    /// `golden` holds each line escaped by [`escape_golden`].
    is_escaped: bool,
    root: &'static str,
    /// Context flags as the tool passes them to `rg`.
    rg_args: &'static [&'static str],
    mode: ContentSearchMode,
    context_before: u32,
    context_after: u32,
    files: Vec<FileHits>,
}

fn cases() -> Vec<Case> {
    let long_line = |path: &str| file(path, vec![matched(1, &first_line(path))]);
    let mut huge = first_line("long/k_huge_5000.txt");
    huge.truncate(MAX_HIT_LINE_BYTES);
    let content = |name: &'static str,
                   golden: &'static [u8],
                   root: &'static str,
                   files: Vec<FileHits>| Case {
        name,
        golden,
        is_escaped: false,
        root,
        rg_args: &[],
        mode: ContentSearchMode::Content,
        context_before: 0,
        context_after: 0,
        files,
    };
    vec![
        content(
            "content",
            include_bytes!("../../../../tests/rg-15.0.0-heading/content.stdout"),
            "tree/order",
            order_files(),
        ),
        Case {
            name: "context_c1",
            golden: include_bytes!("../../../../tests/rg-15.0.0-heading/context_c1.stdout"),
            is_escaped: false,
            root: "tree/ctx",
            rg_args: &["-C", "1"],
            mode: ContentSearchMode::Content,
            context_before: 1,
            context_after: 1,
            files: vec![
                file(
                    "ctx/c.txt",
                    vec![
                        context(1, b"one"),
                        matched(2, b"needle two"),
                        context(3, b"three"),
                        matched(4, b"needle four"),
                        context(5, b"five"),
                        context(8, b"eight"),
                        matched(9, b"needle nine"),
                        context(10, b"ten"),
                        context(11, b"eleven"),
                        matched(12, b"needle twelve"),
                    ],
                ),
                file(
                    "ctx/d.txt",
                    vec![matched(1, b"needle d"), context(2, b"after d")],
                ),
            ],
        },
        Case {
            name: "context_b2_a1",
            golden: include_bytes!("../../../../tests/rg-15.0.0-heading/context_b2_a1.stdout"),
            is_escaped: false,
            root: "tree/ctx",
            rg_args: &["-B", "2", "-A", "1"],
            mode: ContentSearchMode::Content,
            context_before: 2,
            context_after: 1,
            files: vec![
                file(
                    "ctx/c.txt",
                    vec![
                        context(1, b"one"),
                        matched(2, b"needle two"),
                        context(3, b"three"),
                        matched(4, b"needle four"),
                        context(5, b"five"),
                        context(7, b"seven"),
                        context(8, b"eight"),
                        matched(9, b"needle nine"),
                        context(10, b"ten"),
                        context(11, b"eleven"),
                        matched(12, b"needle twelve"),
                    ],
                ),
                file(
                    "ctx/d.txt",
                    vec![matched(1, b"needle d"), context(2, b"after d")],
                ),
            ],
        },
        Case {
            name: "files_with_matches",
            golden: include_bytes!("../../../../tests/rg-15.0.0-heading/files_with_matches.stdout"),
            is_escaped: false,
            root: "tree/order",
            rg_args: &[],
            mode: ContentSearchMode::FilesWithMatches,
            context_before: 0,
            context_after: 0,
            files: order_files()
                .into_iter()
                .map(|hits| FileHits {
                    lines: Vec::new(),
                    ..hits
                })
                .collect(),
        },
        Case {
            name: "count",
            golden: include_bytes!("../../../../tests/rg-15.0.0-heading/count.stdout"),
            is_escaped: false,
            root: "tree/order",
            rg_args: &[],
            mode: ContentSearchMode::Count,
            context_before: 0,
            context_after: 0,
            files: order_files(),
        },
        content(
            "long_lines",
            include_bytes!("../../../../tests/rg-15.0.0-heading/long_lines.stdout"),
            "tree/long",
            vec![
                long_line("long/a_ascii_1500.txt"),
                long_line("long/b_fit_999.txt"),
                long_line("long/c_exact_1000.txt"),
                long_line("long/d_e_acute_600.txt"),
                long_line("long/e_e_acute_1200.txt"),
                long_line("long/g_crlf_cut.txt"),
                long_line("long/h_crlf_fit.txt"),
                long_line("long/i_combining_600.txt"),
                long_line("long/j_combining_1200.txt"),
                file(
                    "long/k_huge_5000.txt",
                    vec![HitLine {
                        is_cut: true,
                        ..matched(1, &huge)
                    }],
                ),
                long_line("long/m_zwj.txt"),
            ],
        ),
        Case {
            name: "invalid_utf8",
            golden: include_bytes!(
                "../../../../tests/rg-15.0.0-heading/invalid_utf8.stdout.escaped"
            ),
            is_escaped: true,
            root: "tree/invalid",
            rg_args: &[],
            mode: ContentSearchMode::Content,
            context_before: 0,
            context_after: 0,
            files: vec![
                long_line("invalid/f_invalid.txt"),
                long_line("invalid/l_invalid_subpart.txt"),
            ],
        },
        Case {
            name: "long_context_lines",
            golden: include_bytes!("../../../../tests/rg-15.0.0-heading/long_context_lines.stdout"),
            is_escaped: false,
            root: "tree/longctx",
            rg_args: &["-C", "1"],
            mode: ContentSearchMode::Content,
            context_before: 1,
            context_after: 1,
            files: vec![file(
                "longctx/lc.txt",
                vec![
                    context(1, &[b'y'; 1200]),
                    matched(2, b"needle"),
                    context(3, &[b'z'; 1001]),
                ],
            )],
        },
        content(
            "empty",
            include_bytes!("../../../../tests/rg-15.0.0-heading/empty.stdout"),
            "tree/empty",
            Vec::new(),
        ),
    ]
}

fn request(root: &str, mode: ContentSearchMode) -> ContentSearchRequest {
    ContentSearchRequest {
        root: PathBuf::from(root),
        root_kind: RootKind::Directory,
        pattern: "needle".to_owned(),
        case: CaseSensitivity::Sensitive,
        globs: Vec::new(),
        file_type: None,
        multiline: MultilineMode::Off,
        context_before: 0,
        context_after: 0,
        mode,
        max_file_bytes: 5 * 1024 * 1024,
        max_columns: 1000,
        result_budget: 201,
        process_cwd: PathBuf::from("/"),
        deadline: tokio::time::Instant::now(),
    }
}

/// `bytes` with each line escaped, so the file is valid UTF-8.
fn escape_golden(bytes: &[u8]) -> Vec<u8> {
    let lines: Vec<String> = bytes
        .split(|&b| b == b'\n')
        .map(|line| line.escape_bytes().to_string())
        .collect();
    lines.join("\n").into_bytes()
}

impl Case {
    /// The bytes `rg` printed.
    fn expected(&self) -> BString {
        if !self.is_escaped {
            return BString::from(self.golden);
        }
        let lines: Vec<Vec<u8>> = self
            .golden
            .split(|&b| b == b'\n')
            .map(|line| Vec::unescape_bytes(line.to_str().expect("escaped golden is UTF-8")))
            .collect();
        BString::from(lines.join(&b'\n'))
    }

    fn golden_file_name(&self) -> String {
        let suffix = if self.is_escaped { ".escaped" } else { "" };
        format!("{}.stdout{suffix}", self.name)
    }
}

fn render(case: &Case) -> BString {
    let search = ContentSearch {
        files: case.files.clone(),
        outcome: ContentSearchOutcome::Complete,
    };
    let request = ContentSearchRequest {
        context_before: case.context_before,
        context_after: case.context_after,
        ..request(case.root, case.mode)
    };
    BString::from(render_rg_heading(&search, &request).expect("renderable hits"))
}

#[test]
fn rendering_matches_rg_15_goldens() {
    for case in cases() {
        assert_eq!(case.expected(), render(&case), "case {}", case.name);
    }
}

/// The goldens are the CI contract (Bazel ships another `rg`); this reruns the
/// cases against a local `rg` 15.0.0 when there is one.
#[tokio::test]
async fn rendering_matches_live_rg_15() {
    let rg = rg_path().expect("rg path");
    let mut version = tokio::process::Command::new(&rg);
    version.arg("--version");
    crate::util::detach_command(&mut version);
    let version = match version.output().await {
        Ok(output) => output.stdout,
        Err(error) => {
            eprintln!("skipping live rg parity: {} failed: {error}", rg.display());
            return;
        }
    };
    if !version.starts_with(b"ripgrep 15.0.0 ") {
        eprintln!(
            "skipping live rg parity: {} is {:?}, not ripgrep 15.0.0",
            rg.display(),
            BString::from(version.lines().next().unwrap_or_default())
        );
        return;
    }
    let tmp = TempDir::new().unwrap();
    write_tree(tmp.path());
    let is_update = std::env::var_os("UPDATE_RG_HEADING_GOLDENS").is_some();
    for case in cases() {
        let mode_flag: &[&str] = match case.mode {
            ContentSearchMode::Content => &[],
            ContentSearchMode::FilesWithMatches => &["-l"],
            ContentSearchMode::Count => &["-c"],
        };
        let mut command = tokio::process::Command::new(&rg);
        command
            .current_dir(tmp.path())
            .args([
                "--no-config",
                "--sort",
                "path",
                "--heading",
                "--with-filename",
                "--line-number",
                "--color=never",
                "--max-columns",
                "1000",
                "--max-columns-preview",
            ])
            .args(case.rg_args)
            .args(mode_flag)
            .args(["-e", "needle", case.root, "--max-filesize", "5M"]);
        crate::util::detach_command(&mut command);
        let stdout = command.output().await.unwrap().stdout;
        if is_update {
            let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/rg-15.0.0-heading")
                .join(case.golden_file_name());
            let bytes = if case.is_escaped {
                escape_golden(&stdout)
            } else {
                stdout.clone()
            };
            std::fs::write(golden, bytes).unwrap();
        }
        assert_eq!(BString::from(stdout), render(&case), "case {}", case.name);
    }
}

/// Known difference: `rg` prints a last line of exactly `max_columns` bytes with
/// no `\n` whole, but the hits cannot tell it from a line that has one.
#[test]
fn last_line_of_max_columns_bytes_is_cut_even_without_newline() {
    let text = [b"needle".as_slice(), &[b'x'; 994]].concat();
    let search = ContentSearch {
        files: vec![file("nonl.txt", vec![matched(1, &text)])],
        outcome: ContentSearchOutcome::Complete,
    };
    let rendered = render_rg_heading(&search, &request("tree", ContentSearchMode::Content));
    let expected = [
        b"tree/nonl.txt\n1:".as_slice(),
        &text,
        b" [... omitted end of long line]\n",
    ]
    .concat();
    assert_eq!(Some(BString::from(expected)), rendered.map(BString::from));
}

/// `rg` would print the first 1000 grapheme clusters, which a cut 4000-byte text
/// of 18-byte emoji clusters does not reach.
#[test]
fn cut_line_without_a_cluster_past_the_preview_is_unrenderable() {
    let mut text = [b"needle ".as_slice(), FAMILY.repeat(300).as_bytes()].concat();
    text.truncate(MAX_HIT_LINE_BYTES);
    let search = ContentSearch {
        files: vec![file(
            "zwj.txt",
            vec![HitLine {
                is_cut: true,
                ..matched(1, &text)
            }],
        )],
        outcome: ContentSearchOutcome::Complete,
    };
    assert_eq!(
        None,
        render_rg_heading(&search, &request("tree", ContentSearchMode::Content))
    );
}

/// `CappedOutput` keeps at most `MAX_STDOUT_BYTES`, so the render stops at the
/// first line past it, and a hit after that point is never looked at.
#[test]
fn render_stops_at_the_first_line_past_the_stdout_cap() {
    let text = [b"needle".as_slice(), &[b'x'; 900]].concat();
    let line_count = MAX_STDOUT_BYTES / text.len() + 10;
    let lines = (1..=line_count as u64)
        .map(|line_no| matched(line_no, &text))
        .collect();
    let mut unrenderable = [b"needle ".as_slice(), FAMILY.repeat(300).as_bytes()].concat();
    unrenderable.truncate(MAX_HIT_LINE_BYTES);
    let search = ContentSearch {
        files: vec![
            file("big.txt", lines),
            file(
                "zwj.txt",
                vec![HitLine {
                    is_cut: true,
                    ..matched(1, &unrenderable)
                }],
            ),
        ],
        outcome: ContentSearchOutcome::Complete,
    };

    let rendered = render_rg_heading(&search, &request("tree", ContentSearchMode::Content))
        .expect("the unrenderable hit lies past the cap");

    assert!(rendered.len() > MAX_STDOUT_BYTES);
    assert!(rendered.len() <= MAX_STDOUT_BYTES + text.len() + 32);
    assert!(rendered.ends_with(b"\n"));
    assert!(!rendered.windows(b"zwj.txt".len()).any(|w| w == b"zwj.txt"));
}
