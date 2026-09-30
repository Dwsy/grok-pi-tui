use crate::model::{PiToolContent, json_text, string};
use agent_client_protocol as acp;
use indexmap::IndexMap;
use serde_json::{Value, json};

pub(crate) fn history_tool_content(content: Vec<PiToolContent>) -> Vec<acp::ToolCallContent> {
    content
        .into_iter()
        .map(|item| match item {
            PiToolContent::Text(text) => {
                acp::ToolCallContent::from(acp::ContentBlock::Text(acp::TextContent::new(text)))
            }
            PiToolContent::Image { data, mime_type } => acp::ToolCallContent::from(
                acp::ContentBlock::Image(acp::ImageContent::new(data, mime_type)),
            ),
        })
        .collect()
}

pub(crate) fn tool_content(value: &Value) -> Vec<acp::ToolCallContent> {
    let source = value.get("content").unwrap_or(value);
    let mut output = Vec::new();
    match source {
        Value::Array(items) => {
            for item in items {
                let kind = string(item, &["type", "kind"])
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if kind == "image"
                    && let (Some(data), Some(mime_type)) = (
                        string(item, &["data"]),
                        string(item, &["mimeType", "mime_type"]),
                    )
                {
                    output.push(acp::ToolCallContent::from(acp::ContentBlock::Image(
                        acp::ImageContent::new(data, mime_type),
                    )));
                } else {
                    let text = string(item, &["text", "content", "message", "output"])
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| json_text(item));
                    if !text.is_empty() {
                        output.push(acp::ToolCallContent::from(acp::ContentBlock::Text(
                            acp::TextContent::new(text),
                        )));
                    }
                }
            }
        }
        _ => {
            let text = json_text(source);
            if !text.is_empty() {
                output.push(acp::ToolCallContent::from(acp::ContentBlock::Text(
                    acp::TextContent::new(text),
                )));
            }
        }
    }
    output
}

#[derive(Clone, Copy)]
struct EditLineNumbers {
    old_line: usize,
    new_line: usize,
}

struct PatchLine {
    text: String,
    old_line: Option<usize>,
    new_line: Option<usize>,
}

struct PatchHunk {
    lines: Vec<PatchLine>,
}

/// Convert Pi's edit/write input contract into ACP's native diff payload.
///
/// The Pager's Edit card and viewer intentionally render only `Diff` content;
/// ordinary text results do not provide the old/new source needed for a hunk.
pub(crate) fn edit_diff_content(
    tool_name: &str,
    args: Option<&Value>,
    result: Option<&Value>,
) -> Option<Vec<acp::ToolCallContent>> {
    if tool_kind(tool_name) != acp::ToolKind::Edit {
        return None;
    }
    let args = args?;
    let path = string(args, &["path", "filePath", "file_path", "target_file"])?;
    let line_numbers = edit_line_numbers(args, result);
    if let Some(edits) = args.get("edits").and_then(Value::as_array) {
        let diffs = edits
            .iter()
            .enumerate()
            .filter_map(|(index, edit)| {
                let old_text = string(edit, &["oldText", "old_text"])?;
                let new_text = string(edit, &["newText", "new_text"])?;
                Some(edit_diff(
                    path,
                    Some(old_text),
                    new_text,
                    line_numbers.get(index).copied().flatten(),
                ))
            })
            .collect::<Vec<_>>();
        return (!diffs.is_empty()).then_some(diffs);
    }
    let new_text = string(args, &["newText", "new_text", "content"])?;
    let old_text = string(args, &["oldText", "old_text"]);
    Some(vec![edit_diff(
        path,
        old_text,
        new_text,
        line_numbers.first().copied().flatten(),
    )])
}

fn edit_diff(
    path: &str,
    old_text: Option<&str>,
    new_text: &str,
    line_numbers: Option<EditLineNumbers>,
) -> acp::ToolCallContent {
    let mut diff =
        acp::Diff::new(path, new_text.to_owned()).old_text(old_text.map(ToOwned::to_owned));
    if let Some(line_numbers) = line_numbers {
        let meta = json!({
            "old_line": line_numbers.old_line,
            "new_line": line_numbers.new_line,
        })
        .as_object()
        .cloned();
        diff = diff.meta(meta);
    }
    acp::ToolCallContent::Diff(diff)
}

fn edit_line_numbers(args: &Value, result: Option<&Value>) -> Vec<Option<EditLineNumbers>> {
    let Some(patch) = result
        .and_then(|result| {
            result
                .pointer("/details/patch")
                .or_else(|| result.get("patch"))
        })
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    let hunks = parse_patch_hunks(patch);
    edit_replacements(args)
        .into_iter()
        .map(|(old_text, _)| find_edit_line_numbers(&hunks, old_text))
        .collect()
}

fn edit_replacements(args: &Value) -> Vec<(&str, &str)> {
    if let Some(edits) = args.get("edits").and_then(Value::as_array) {
        return edits
            .iter()
            .filter_map(|edit| {
                Some((
                    string(edit, &["oldText", "old_text"])?,
                    string(edit, &["newText", "new_text"])?,
                ))
            })
            .collect();
    }
    match (
        string(args, &["oldText", "old_text"]),
        string(args, &["newText", "new_text"]),
    ) {
        (Some(old_text), Some(new_text)) => vec![(old_text, new_text)],
        _ => Vec::new(),
    }
}

fn parse_patch_hunks(patch: &str) -> Vec<PatchHunk> {
    let mut hunks = Vec::new();
    let mut current: Option<(PatchHunk, usize, usize)> = None;

    for line in patch.lines() {
        if let Some((old_line, new_line)) = parse_patch_hunk_header(line) {
            if let Some((hunk, _, _)) = current.take() {
                hunks.push(hunk);
            }
            current = Some((PatchHunk { lines: Vec::new() }, old_line, new_line));
            continue;
        }
        let Some((hunk, old_line, new_line)) = current.as_mut() else {
            continue;
        };
        let Some((prefix, text)) = line.split_at_checked(1) else {
            continue;
        };
        match prefix {
            " " => {
                hunk.lines.push(PatchLine {
                    text: text.to_string(),
                    old_line: Some(*old_line),
                    new_line: Some(*new_line),
                });
                *old_line += 1;
                *new_line += 1;
            }
            "-" => {
                hunk.lines.push(PatchLine {
                    text: text.to_string(),
                    old_line: Some(*old_line),
                    new_line: None,
                });
                *old_line += 1;
            }
            "+" => {
                hunk.lines.push(PatchLine {
                    text: text.to_string(),
                    old_line: None,
                    new_line: Some(*new_line),
                });
                *new_line += 1;
            }
            _ => {}
        }
    }
    if let Some((hunk, _, _)) = current {
        hunks.push(hunk);
    }
    hunks
}

fn parse_patch_hunk_header(line: &str) -> Option<(usize, usize)> {
    let ranges = line.strip_prefix("@@ ")?.split_once(" @@")?.0;
    let mut ranges = ranges.split_whitespace();
    let old_line = parse_patch_range_start(ranges.next()?)?;
    let new_line = parse_patch_range_start(ranges.next()?)?;
    Some((old_line, new_line))
}

fn parse_patch_range_start(range: &str) -> Option<usize> {
    range
        .strip_prefix(['-', '+'])?
        .split_once(',')
        .map(|(start, _)| start)
        .unwrap_or(range.strip_prefix(['-', '+'])?)
        .parse()
        .ok()
}

fn find_edit_line_numbers(hunks: &[PatchHunk], old_text: &str) -> Option<EditLineNumbers> {
    for hunk in hunks {
        let Some((line_index, old_line)) = find_hunk_line(hunk, old_text, false) else {
            continue;
        };
        let new_line = hunk_new_line_for_old_position(hunk, line_index)?;
        return Some(EditLineNumbers { old_line, new_line });
    }
    None
}

fn hunk_new_line_for_old_position(hunk: &PatchHunk, line_index: usize) -> Option<usize> {
    hunk.lines[line_index]
        .new_line
        .or_else(|| {
            hunk.lines[line_index..]
                .iter()
                .find_map(|line| line.new_line)
        })
        .or_else(|| {
            hunk.lines[..line_index]
                .iter()
                .rev()
                .find_map(|line| line.new_line.map(|line_number| line_number + 1))
        })
}

fn find_hunk_line(hunk: &PatchHunk, needle: &str, new_side: bool) -> Option<(usize, usize)> {
    let needle = needle.replace("\r\n", "\n").replace('\r', "\n");
    let needle = needle.trim_end_matches('\n');
    if needle.is_empty() {
        return None;
    }

    let mut text = String::new();
    let mut starts = Vec::new();
    for (index, line) in hunk.lines.iter().enumerate() {
        let line_number = if new_side {
            line.new_line
        } else {
            line.old_line
        };
        let Some(line_number) = line_number else {
            continue;
        };
        starts.push((text.len(), index, line_number));
        text.push_str(&line.text);
        text.push('\n');
    }
    let offset = text.find(needle)?;
    starts
        .into_iter()
        .take_while(|(start, _, _)| *start <= offset)
        .last()
        .map(|(_, index, line_number)| (index, line_number))
}

/// Classify a Pi tool into the ACP kind the native Pager cards dispatch on.
///
/// Pi's built-in tool set is exact (`read`, `bash`, `edit`, `write`, `grep`,
/// `find`, `ls`), so classification is exact-name — never substring matching.
/// A substring test like `name.contains("search")` would misclassify
/// extension/MCP/pi tools such as `web_search`, `memory_search`,
/// `session_search`, or `x_search` as grep searches and route their results
/// through `path:line` parsing. Any unrecognised name stays `Other` and
/// renders as a generic tool card.
pub(crate) fn tool_kind(name: &str) -> acp::ToolKind {
    let name = name.trim().to_ascii_lowercase();
    match name.as_str() {
        // Pi builtin read/bash/edit-family tools plus their common aliases.
        "read" | "read_file" | "view" => acp::ToolKind::Read,
        "bash" | "execute" | "run_terminal_command" => acp::ToolKind::Execute,
        "eval" => acp::ToolKind::Other,
        "edit" | "write" | "create_file" => acp::ToolKind::Edit,
        "grep" | "find" | "glob" => acp::ToolKind::Search,
        // ListDir is detected in the pager via `raw_input.target_directory`
        // (there is no ACP ListDir kind). Keep Other so that branch can match.
        "ls" | "list_dir" | "listdir" | "list_directory" => acp::ToolKind::Other,
        "web_fetch" | "fetch" => acp::ToolKind::Fetch,
        _ => acp::ToolKind::Other,
    }
}

fn is_find_tool(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "find" || name == "glob" || name.ends_with("_find") || name.contains("glob_file")
}

fn is_ls_tool(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "ls" || name == "list_dir" || name == "listdir" || name == "list_directory"
}

/// Rewrite Pi tool args into shapes the native Grok cards already understand.
///
/// - `write` → `variant: "Write"` so Edit cards show "Creating "
/// - `ls` → `target_directory` (ListDir card key)
/// - `grep` → `-i` alias for `ignoreCase`
/// - `find` → `output_mode: files_with_matches`
/// - `bash` / execute → `task_name` → `description` (Pager Execute card title)
///
/// Other/generic tools (including `fabric_exec`) keep raw args unchanged; the
/// pager's F2 `show_other_tool_args` decides whether expanded cards show them.
pub(crate) fn normalize_tool_raw_input(name: &str, args: Option<Value>) -> Option<Value> {
    let mut args = args?;
    let Some(obj) = args.as_object_mut() else {
        return Some(args);
    };
    let lower = name.to_ascii_lowercase();

    if lower == "eval" {
        obj.entry("variant".to_string())
            .or_insert_with(|| json!("Eval"));
        return Some(args);
    }

    if lower == "codemode" {
        obj.entry("variant".to_string())
            .or_insert_with(|| json!("Codemode"));
        return Some(args);
    }

    if lower == "write" || lower.ends_with("_write") {
        obj.entry("variant".to_string())
            .or_insert_with(|| json!("Write"));
    }

    if is_ls_tool(name) {
        if let Some(path) = obj.get("path").cloned() {
            obj.entry("target_directory".to_string()).or_insert(path);
        } else {
            obj.entry("target_directory".to_string())
                .or_insert_with(|| json!("."));
        }
    }

    if is_find_tool(name) {
        obj.entry("output_mode".to_string())
            .or_insert_with(|| json!("files_with_matches"));
        // Prefer `pattern` as the search term; copy glob-like patterns into
        // `glob_pattern` for extractors that look for it.
        if let Some(pattern) = obj.get("pattern").cloned() {
            obj.entry("glob_pattern".to_string()).or_insert(pattern);
        }
    }

    if lower == "grep" {
        if let Some(ignore_case) = obj.get("ignoreCase").cloned() {
            obj.entry("-i".to_string()).or_insert(ignore_case);
        }
    }

    // pi-grok-bash uses `task_name`; stock Grok + Pager Execute cards read
    // `raw_input.description` for the human-readable title (spinner / header).
    if tool_kind(name) == acp::ToolKind::Execute {
        let missing_or_empty_description = obj
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none();
        if missing_or_empty_description
            && let Some(label) = obj
                .get("task_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        {
            obj.insert("description".to_string(), json!(label));
        }
    }

    Some(args)
}

/// Project Pi tool results into the typed `raw_output` shapes native Grok cards
/// deserialize (`ToolOutput::ReadFile` / `Bash` / `GrepSearch` / `ListDir`).
///
/// Without this conversion the Read card has no path/line metadata, the
/// Execute card/viewer has command only, and Search/ListDir cards show empty
/// structured results — Pi's payload is text `content`, not Grok's tagged
/// tool output enum.
pub(crate) fn normalize_tool_raw_output(
    name: &str,
    args: Option<&Value>,
    result: &Value,
    is_error: bool,
) -> Value {
    if name.eq_ignore_ascii_case("eval") {
        return result.clone();
    }
    if name.eq_ignore_ascii_case("codemode") {
        return codemode_tool_output(result);
    }
    if is_ls_tool(name) {
        return ls_tool_output(args, result, is_error);
    }
    match tool_kind(name) {
        acp::ToolKind::Read => read_tool_output(args, result, is_error),
        acp::ToolKind::Execute => {
            let command = args
                .and_then(|value| string(value, &["command", "cmd"]))
                .unwrap_or_default()
                .to_string();
            let description = args.and_then(|value| {
                string(value, &["description", "task_name"])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
            });
            bash_tool_output(&command, description.as_deref(), result, is_error)
        }
        acp::ToolKind::Search => {
            if is_find_tool(name) {
                find_tool_output(result, is_error)
            } else {
                grep_tool_output(result, is_error)
            }
        }
        _ => result.clone(),
    }
}

/// Project Pi `codemode` results into the canonical `Codemode` raw output the
/// native card deserializes: nested call records, header-stripped script output,
/// and the spilled full-output path.
///
/// Shapes handled (Pi side, `extensions/codemode/execute.ts`):
/// - live end: `{ content: [...], details: { calls, fullOutputPath? } }`
/// - live update (partialResult): `{ content: [], details: { calls } }`
/// - replay: history stores `details` alone as raw_output, so callers fold the
///   persisted content blocks back in as `content` (see the adapter replay path).
pub(crate) fn codemode_tool_output(result: &Value) -> Value {
    let details = result.get("details").unwrap_or(result);
    let calls = details.get("calls").cloned().unwrap_or_else(|| json!([]));
    json!({
        "type": "Codemode",
        "calls": calls,
        "output": strip_codemode_script_header(&codemode_output_text(result)),
        "full_output_path": details
            .get("fullOutputPath")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    })
}

/// Text items of a Pi tool result without falling back to JSON-dumping the whole
/// payload: `codemode` replay raw_output is the `details` object itself, and
/// serializing its `calls` would masquerade as output text.
///
/// The `Script completed|failed` header arrives as its own content item (or leads
/// the merged truncated text); it is dropped like Pi's own renderer does, and the
/// remainder is trimmed on both ends.
fn codemode_output_text(result: &Value) -> String {
    let Some(content) = result.get("content") else {
        return String::new();
    };
    let items: Vec<&str> = match content {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect(),
        Value::String(text) => vec![text.as_str()],
        _ => Vec::new(),
    };
    let body = items
        .into_iter()
        .filter(|text| !is_standalone_script_header(text))
        .collect::<Vec<_>>()
        .join("\n");
    strip_codemode_script_header(&body).trim().to_string()
}

/// Full standalone Pi script header: `Script completed|failed\nWall time N seconds\nOutput:\n`.
/// A merged truncated item (header followed by real output) strips to non-empty and stays.
fn is_standalone_script_header(text: &str) -> bool {
    strip_codemode_script_header(text).is_empty()
}

/// Strip Pi's `Script completed|failed\nWall time N seconds\nOutput:\n` header
/// from a codemode script result. Rejected input (invalid options) has no header.
fn strip_codemode_script_header(text: &str) -> &str {
    let mut lines = text.splitn(3, '\n');
    let (Some(status), Some(wall), Some(rest)) = (lines.next(), lines.next(), lines.next()) else {
        return text;
    };
    let header_status = status == "Script completed" || status == "Script failed";
    let header_wall = wall
        .strip_prefix("Wall time ")
        .and_then(|value| value.strip_suffix(" seconds"))
        .is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        });
    if !header_status || !header_wall {
        return text;
    }
    match rest.strip_prefix("Output:\n") {
        Some(body) => body,
        None if rest == "Output:" => "",
        _ => text,
    }
}

pub(crate) fn pi_result_text(value: &Value) -> String {
    if let Some(text) = value.get("output").and_then(Value::as_str) {
        return text.to_string();
    }
    let source = value.get("content").unwrap_or(value);
    match source {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                string(item, &["text", "content", "message", "output"])
                    .map(str::to_owned)
                    .or_else(|| item.as_str().map(str::to_owned))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::String(text) => text.clone(),
        _ => {
            let text = json_text(source);
            if text == "null" { String::new() } else { text }
        }
    }
}

fn read_tool_output(args: Option<&Value>, result: &Value, is_error: bool) -> Value {
    let text = pi_result_text(result);
    if is_error {
        let message = if text.trim().is_empty() {
            "Read failed".to_string()
        } else {
            text
        };
        return json!({
            "type": "ReadFile",
            "FileReadError": message,
        });
    }

    let path = args
        .and_then(|value| string(value, &["path", "filePath", "file_path", "target_file"]))
        .unwrap_or_default()
        .to_string();
    let offset = args
        .and_then(|value| value.get("offset"))
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    let limit = args
        .and_then(|value| value.get("limit"))
        .and_then(Value::as_u64)
        .map(|n| n as usize);

    // Strip Pi continuation footers for line counting; keep full text for content.
    let body = text
        .split("\n\n[")
        .next()
        .unwrap_or(text.as_str())
        .trim_end_matches('\n');
    let content_lines = if body.is_empty() {
        0
    } else {
        body.lines().count()
    };
    let total_from_footer = text.rsplit_once(" of ").and_then(|(_, rest)| {
        rest.split(|c: char| !c.is_ascii_digit())
            .find(|part| !part.is_empty())
            .and_then(|digits| digits.parse::<usize>().ok())
    });
    let start_index = offset.unwrap_or(1).saturating_sub(1);
    let total_lines = total_from_footer
        .unwrap_or(start_index.saturating_add(content_lines))
        .max(content_lines);

    // Pager Read cards treat FileContent.offset as a 0-based skip count
    // (`start = offset + 1`). Pi's offset is 1-indexed. When Pi omits a window,
    // still publish a 0-based full-file range so the header can show line counts.
    let (stored_offset, stored_limit) = match (offset, limit) {
        (None, None) if content_lines > 0 => (Some(0usize), Some(content_lines)),
        (offset, limit) => (offset.map(|value| value.saturating_sub(1)), limit),
    };

    json!({
        "type": "ReadFile",
        "FileContent": {
            "content": text,
            "absolute_path": path,
            "offset": stored_offset,
            "limit": stored_limit,
            "raw_output": body,
            "total_lines": total_lines,
        }
    })
}

/// Project Pi `grep` text (`path:line: content`) into `ToolOutput::GrepSearch`.
fn grep_tool_output(result: &Value, is_error: bool) -> Value {
    let text = pi_result_text(result);
    let trimmed = text.trim();
    if is_error {
        return json!({
            "type": "GrepSearch",
            "stdout": text.as_bytes().to_vec(),
            "stderr": text.as_bytes().to_vec(),
            "exit_code": 2,
            "match_count": 0,
            "file_matches": [],
        });
    }
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("No matches found") {
        return json!({
            "type": "GrepSearch",
            "stdout": text.as_bytes().to_vec(),
            "stderr": Vec::<u8>::new(),
            "exit_code": 1,
            "match_count": 0,
            "file_matches": [],
        });
    }

    let (file_matches, match_count) = parse_pi_grep_matches(&text);
    json!({
        "type": "GrepSearch",
        "stdout": text.as_bytes().to_vec(),
        "stderr": Vec::<u8>::new(),
        "exit_code": 0,
        "match_count": match_count,
        "file_matches": file_matches,
    })
}

/// Project Pi `find` path list into `ToolOutput::GrepSearch` (files_with_matches).
fn find_tool_output(result: &Value, is_error: bool) -> Value {
    let text = pi_result_text(result);
    let trimmed = text.trim();
    if is_error {
        return json!({
            "type": "GrepSearch",
            "stdout": text.as_bytes().to_vec(),
            "stderr": text.as_bytes().to_vec(),
            "exit_code": 2,
            "match_count": 0,
            "file_matches": [],
        });
    }
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("No files found matching pattern")
        || trimmed.eq_ignore_ascii_case("No files found")
    {
        return json!({
            "type": "GrepSearch",
            "stdout": text.as_bytes().to_vec(),
            "stderr": Vec::<u8>::new(),
            "exit_code": 0,
            "match_count": 0,
            "file_matches": [],
        });
    }

    // One path per line. Store as stdout so the pager can also recover
    // file_paths via parse_file_paths_from_stdout when file_matches is empty.
    let paths: Vec<&str> = trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let match_count = paths.len();
    json!({
        "type": "GrepSearch",
        "stdout": text.as_bytes().to_vec(),
        "stderr": Vec::<u8>::new(),
        "exit_code": 0,
        "match_count": match_count,
        "file_matches": [],
    })
}

/// Project Pi `ls` listing into `ToolOutput::ListDir`.
fn ls_tool_output(args: Option<&Value>, result: &Value, is_error: bool) -> Value {
    let text = pi_result_text(result);
    let path = args
        .and_then(|value| string(value, &["path", "target_directory", "targetDirectory"]))
        .unwrap_or(".")
        .to_string();

    if is_error {
        let message = if text.trim().is_empty() {
            "List directory failed".to_string()
        } else {
            text
        };
        // Prefer NotFound when Pi's message looks like a missing path.
        if message.to_ascii_lowercase().contains("not found")
            || message.to_ascii_lowercase().contains("no such file")
        {
            return json!({ "type": "ListDir", "NotFound": message });
        }
        if message.to_ascii_lowercase().contains("not a directory") {
            return json!({ "type": "ListDir", "NotADirectory": message });
        }
        if message.to_ascii_lowercase().contains("permission") {
            return json!({ "type": "ListDir", "PermissionDenied": message });
        }
        return json!({ "type": "ListDir", "Error": message });
    }

    json!({
        "type": "ListDir",
        "Content": {
            "content": text,
            "absolute_root_path": path,
        }
    })
}

/// Parse Pi grep output lines:
/// - direct match: `path:line: content`
/// - RTK compact match: `line: content` under `> path (N matches):`
/// - context: `path-line- content` (ignored for match_count)
fn parse_pi_grep_matches(text: &str) -> (Vec<Value>, usize) {
    let mut order: Vec<String> = Vec::new();
    let mut by_path: IndexMap<String, Vec<Value>> = IndexMap::new();
    let mut match_count = 0usize;
    let mut current_path: Option<String> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(path) = parse_rtk_grep_header(line) {
            current_path = Some(path.to_string());
            continue;
        }
        let parsed = split_pi_grep_match_line(line).or_else(|| {
            current_path.as_deref().and_then(|path| {
                split_rtk_grep_match_line(line)
                    .map(|(line_number, content)| (path, line_number, content))
            })
        });
        if let Some((path, line_number, content)) = parsed {
            match_count += 1;
            if !by_path.contains_key(path) {
                order.push(path.to_string());
            }
            by_path.entry(path.to_string()).or_default().push(json!({
                "line_number": line_number,
                "content": content,
            }));
        }
    }

    let file_matches = order
        .into_iter()
        .filter_map(|path| {
            let matches = by_path.swap_remove(&path)?;
            Some(json!({ "path": path, "matches": matches }))
        })
        .collect();
    (file_matches, match_count)
}

fn parse_rtk_grep_header(line: &str) -> Option<&str> {
    let header = line.strip_prefix("> ")?.strip_suffix(':')?;
    let (path, count) = header.rsplit_once(" (")?;
    count
        .strip_suffix(" matches)")
        .or_else(|| count.strip_suffix(" match)"))
        .and_then(|value| value.parse::<usize>().ok())
        .map(|_| path)
}

fn split_rtk_grep_match_line(line: &str) -> Option<(usize, &str)> {
    let line = line.trim_start();
    let (line_number, content) = line.split_once(':')?;
    let line_number = line_number.parse().ok()?;
    Some((line_number, content.trim_start()))
}

fn split_pi_grep_match_line(line: &str) -> Option<(&str, usize, &str)> {
    // Format: `relative/path:12: content` — path may contain colons on Windows
    // (`C:\...`), so scan from the right for `:digits:`.
    let bytes = line.as_bytes();
    let mut i = bytes.len();
    // Find last ": <content>" separator after a line number.
    while i > 0 {
        // find `:` that starts content
        if let Some(colon_content) = line[..i].rfind(':') {
            let after = &line[colon_content + 1..];
            // content may start with space
            let before = &line[..colon_content];
            if let Some(colon_line) = before.rfind(':') {
                let line_str = &before[colon_line + 1..];
                if let Ok(line_number) = line_str.parse::<usize>() {
                    let path = &before[..colon_line];
                    if !path.is_empty() && line_number > 0 {
                        let content = after.strip_prefix(' ').unwrap_or(after);
                        return Some((path, line_number, content));
                    }
                }
            }
            i = colon_content;
        } else {
            break;
        }
    }
    None
}

pub(crate) fn bash_tool_output(
    command: &str,
    description: Option<&str>,
    result: &Value,
    is_error: bool,
) -> Value {
    let text = if result.get("output").and_then(Value::as_str).is_some()
        && result.get("content").is_none()
    {
        // Direct Pi `bash` RPC response: { output, exitCode, ... }.
        result
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        pi_result_text(result)
    };

    let exit_code = result
        .get("exitCode")
        .and_then(Value::as_i64)
        .or_else(|| {
            result
                .get("details")
                .and_then(|details| details.get("exitCode"))
                .and_then(Value::as_i64)
        })
        .unwrap_or(if is_error { 1 } else { 0 });

    let truncated = result
        .get("truncated")
        .and_then(Value::as_bool)
        .or_else(|| {
            result
                .pointer("/details/truncation/truncated")
                .and_then(Value::as_bool)
        })
        .unwrap_or(false);

    let output_file = result
        .get("fullOutputPath")
        .and_then(Value::as_str)
        .or_else(|| {
            result
                .pointer("/details/fullOutputPath")
                .and_then(Value::as_str)
        })
        .unwrap_or("")
        .to_string();

    // Prefer arg label; fall back to tool-result details (background bridge).
    let description = description
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            result
                .pointer("/details/description")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
        });

    let bytes = text.as_bytes().to_vec();
    let total_bytes = bytes.len();
    json!({
        "type": "Bash",
        "output": bytes,
        "output_for_prompt": text,
        "exit_code": exit_code,
        "command": command,
        "truncated": truncated,
        "signal": null,
        "timed_out": false,
        "description": description,
        "current_dir": "",
        "output_file": output_file,
        "total_bytes": total_bytes,
        "was_bare_echo": false,
    })
}

#[cfg(test)]
mod tests;
