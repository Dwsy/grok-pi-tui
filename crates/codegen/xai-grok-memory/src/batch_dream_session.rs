//! Each read label is bound to the file's hash at read time, so a plan can only
//! edit text the model actually saw; reads share a per-batch byte budget so topic
//! size never grows the model context.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::batch_dream::{
    BatchCommit, BatchDreamError, BatchDreamStore, BatchLease, BatchReport, MAX_PATH_BYTES, Result,
    TOPICS_DIR,
};
use crate::batch_dream_catalog::TopicCatalog;
use crate::batch_dream_io::{hash_file, read_range};
use crate::batch_dream_outline::{Section, heading_level, is_fence, outline, scan_lines};

const WHOLE_TOPIC_BYTES: u64 = 16 * 1024;
pub const MAX_RANGE_BYTES: u64 = 32 * 1024;
const MAX_OUTLINE_SECTIONS: usize = 300;
pub const MAX_ACTIONS_PER_CALL: usize = 12;
const MAX_PATTERNS: usize = 8;
const MAX_PATTERN_BYTES: usize = 256;
const MAX_SEARCH_PATHS: usize = 64;
const MAX_HITS_PER_PATTERN: usize = 20;
const MAX_HITS_PER_SEARCH: usize = 60;
const MAX_SNIPPET_BYTES: usize = 300;
const REGEX_SIZE_LIMIT: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum BatchAction {
    ReadTopic {
        path: String,
    },
    ReadRange {
        path: String,
        start: u64,
        max_bytes: u64,
    },
    Search {
        patterns: Vec<String>,
        paths: Vec<String>,
        after: Option<SearchCursor>,
    },
    List {
        after: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchCursor {
    pub path: String,
    pub offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionBudgets {
    pub read_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadRecord {
    pub(crate) label: String,
    pub(crate) path: String,
    pub(crate) file_hash: String,
    pub(crate) start: u64,
    pub(crate) text: Option<String>,
    pub(crate) sections: Vec<Section>,
}

#[derive(Debug)]
pub struct BatchDreamSession {
    pub(crate) store: Arc<BatchDreamStore>,
    pub(crate) catalog: Arc<TopicCatalog>,
    pub(crate) lease: BatchLease,
    pub(crate) reads: Vec<ReadRecord>,
    read_bytes: usize,
    budgets: SessionBudgets,
}

impl BatchDreamSession {
    #[must_use]
    pub fn new(
        store: Arc<BatchDreamStore>,
        catalog: Arc<TopicCatalog>,
        lease: BatchLease,
        budgets: SessionBudgets,
    ) -> BatchDreamSession {
        BatchDreamSession {
            store,
            catalog,
            lease,
            reads: Vec::new(),
            read_bytes: 0,
            budgets,
        }
    }

    #[must_use]
    pub fn lease(&self) -> &BatchLease {
        &self.lease
    }

    #[must_use]
    pub fn read_bytes(&self) -> usize {
        self.read_bytes
    }

    #[must_use]
    pub fn render_notes(&self) -> String {
        let mut rendered = String::new();
        for (index, note) in self.lease.notes.iter().enumerate() {
            let date = chrono::DateTime::from_timestamp(note.created_at, 0).map_or_else(
                || "unknown date".to_owned(),
                |time| time.format("%Y-%m-%d %H:%M UTC").to_string(),
            );
            rendered.push_str(&format!(
                "<note label=\"N{}\" date=\"{date}\" source=\"{}\">\n{}\n</note>\n",
                index + 1,
                note.path,
                note.content.trim_end()
            ));
        }
        rendered
    }

    pub fn dispatch(&mut self, actions: &[BatchAction], now: i64) -> Result<Value> {
        self.validate_lease(now)?;
        let mut results = Vec::with_capacity(actions.len());
        for (index, action) in actions.iter().enumerate() {
            self.store.control.check()?;
            let result = if index >= MAX_ACTIONS_PER_CALL {
                Err(BatchDreamError::Invalid(format!(
                    "at most {MAX_ACTIONS_PER_CALL} actions run per call"
                )))
            } else {
                self.run_action(action)
            };
            results.push(match result {
                Ok(value) => value,
                Err(error @ (BatchDreamError::Interrupted | BatchDreamError::Database(_))) => {
                    return Err(error);
                }
                Err(error) => json!({"error": error.to_string()}),
            });
        }
        self.validate_lease(now)?;
        Ok(Value::Array(results))
    }

    pub fn refresh(&mut self, paths: &BTreeSet<String>) -> Result<Value> {
        let mut results = Vec::new();
        for path in paths.iter().take(MAX_ACTIONS_PER_CALL) {
            results.push(match self.read_topic(path) {
                Ok(value) => value,
                Err(error @ (BatchDreamError::Interrupted | BatchDreamError::Database(_))) => {
                    return Err(error);
                }
                Err(error) => json!({"path": path, "error": error.to_string()}),
            });
        }
        Ok(Value::Array(results))
    }

    pub fn commit(&self, commit: &BatchCommit, now: i64) -> Result<BatchReport> {
        self.store.commit(&self.lease, commit, now)
    }

    pub fn release(&self, reason: &str, now: i64) -> Result<BatchReport> {
        self.store.release(&self.lease, reason, now)
    }

    fn validate_lease(&self, now: i64) -> Result<()> {
        self.store
            .validate_lease(&self.store.connection()?, &self.lease, now)
    }

    fn run_action(&mut self, action: &BatchAction) -> Result<Value> {
        match action {
            BatchAction::ReadTopic { path } => self.read_topic(path),
            BatchAction::ReadRange {
                path,
                start,
                max_bytes,
            } => self.read_text_range(path, *start, *max_bytes),
            BatchAction::Search {
                patterns,
                paths,
                after,
            } => self.search(patterns, paths, after.as_ref()),
            BatchAction::List { after } => {
                let (entries, next) = self.catalog.page(after.as_deref());
                Ok(json!({"action": "list", "entries": entries, "next": next}))
            }
        }
    }

    fn existing_topic(&self, path: &str) -> Result<std::path::PathBuf> {
        let absolute = self
            .store
            .validate_topic_path(&self.store.connection()?, path)?;
        if !absolute.is_file() {
            return Err(BatchDreamError::Invalid(format!("{path} does not exist")));
        }
        Ok(absolute)
    }

    fn charge(&mut self, bytes: usize) -> Result<()> {
        let total = self.read_bytes.saturating_add(bytes);
        if total > self.budgets.read_bytes {
            return Err(BatchDreamError::Invalid(format!(
                "read budget of {} bytes is spent ({} used); plan with what you have or defer",
                self.budgets.read_bytes, self.read_bytes
            )));
        }
        self.read_bytes = total;
        Ok(())
    }

    fn next_label(&self) -> String {
        format!("R{}", self.reads.len() + 1)
    }

    fn read_topic(&mut self, path: &str) -> Result<Value> {
        let absolute = self.existing_topic(path)?;
        let store = Arc::clone(&self.store);
        let control = &store.control;
        let file_hash = hash_file(&absolute, control)?;
        let size = std::fs::metadata(&absolute)?.len();
        let label = self.next_label();
        let (value, cost, text, sections) = if size <= WHOLE_TOPIC_BYTES {
            let (text, _) = read_range(&absolute, 0, WHOLE_TOPIC_BYTES)?;
            let value = json!({"action": "read_topic", "label": label, "path": path, "bytes": size, "text": text});
            (value, text.len(), Some(text), Vec::new())
        } else {
            let (sections, is_truncated) = outline(&absolute, control, MAX_OUTLINE_SECTIONS)?;
            let value = json!({
                "action": "read_topic", "label": label, "path": path, "bytes": size,
                "outline": sections, "outline_truncated": is_truncated,
                "note": "Large topic: read sections with read_range using a section's start and size."
            });
            let cost = value.to_string().len();
            (value, cost, None, sections)
        };
        if hash_file(&absolute, control)? != file_hash {
            return Err(BatchDreamError::Conflict(format!(
                "{path} changed while reading"
            )));
        }
        self.charge(cost)?;
        self.reads.push(ReadRecord {
            label,
            path: path.to_owned(),
            file_hash,
            start: 0,
            text,
            sections,
        });
        Ok(value)
    }

    fn read_text_range(&mut self, path: &str, start: u64, max_bytes: u64) -> Result<Value> {
        let absolute = self.existing_topic(path)?;
        let store = Arc::clone(&self.store);
        let control = &store.control;
        let file_hash = hash_file(&absolute, control)?;
        let (text, end) = read_range(&absolute, start, max_bytes.clamp(1, MAX_RANGE_BYTES))?;
        if hash_file(&absolute, control)? != file_hash {
            return Err(BatchDreamError::Conflict(format!(
                "{path} changed while reading"
            )));
        }
        self.charge(text.len())?;
        let size = std::fs::metadata(&absolute)?.len();
        let label = self.next_label();
        let value = json!({
            "action": "read_range", "label": label, "path": path, "start": start, "end": end,
            "bytes": size, "more": end < size, "text": text
        });
        self.reads.push(ReadRecord {
            label,
            path: path.to_owned(),
            file_hash,
            start,
            text: Some(text),
            sections: Vec::new(),
        });
        Ok(value)
    }

    fn search(
        &self,
        patterns: &[String],
        paths: &[String],
        after: Option<&SearchCursor>,
    ) -> Result<Value> {
        if patterns.is_empty()
            || patterns.len() > MAX_PATTERNS
            || patterns
                .iter()
                .any(|pattern| pattern.len() > MAX_PATTERN_BYTES)
            || paths.len() > MAX_SEARCH_PATHS
        {
            return Err(BatchDreamError::Invalid(format!(
                "search takes 1..={MAX_PATTERNS} patterns of at most {MAX_PATTERN_BYTES} bytes and {MAX_SEARCH_PATHS} paths"
            )));
        }
        let regexes = patterns
            .iter()
            .map(|pattern| {
                regex::RegexBuilder::new(pattern)
                    .size_limit(REGEX_SIZE_LIMIT)
                    .dfa_size_limit(REGEX_SIZE_LIMIT)
                    .build()
                    .map_err(|error| {
                        BatchDreamError::Invalid(format!("bad pattern {pattern:?}: {error}"))
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        let files = if paths.is_empty() {
            self.topic_paths()?
        } else {
            let mut files = paths
                .iter()
                .map(|path| self.existing_topic(path).map(|_| path.clone()))
                .collect::<Result<Vec<_>>>()?;
            files.sort();
            files.dedup();
            files
        };
        let control = &self.store.control;
        let mut counts = vec![0usize; regexes.len()];
        let mut hits = Vec::new();
        let mut next = None;
        for path in files {
            if after.is_some_and(|cursor| path < cursor.path) {
                continue;
            }
            let resume_at = after
                .filter(|cursor| cursor.path == path)
                .map(|cursor| cursor.offset);
            let mut heading = String::new();
            let mut is_fenced = false;
            scan_lines(&self.store.scope_dir.join(&path), control, |line| {
                if is_fence(line.prefix) {
                    is_fenced = !is_fenced;
                } else if !is_fenced && heading_level(line.prefix).is_some() {
                    heading = String::from_utf8_lossy(line.prefix).trim_end().to_owned();
                }
                if resume_at.is_some_and(|offset| line.start < offset) {
                    return Ok(true);
                }
                let text = String::from_utf8_lossy(line.prefix);
                let line_hits: Vec<(usize, usize)> = regexes
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| {
                        counts
                            .get(*index)
                            .is_some_and(|count| *count < MAX_HITS_PER_PATTERN)
                    })
                    .filter_map(|(index, regex)| {
                        regex.find(&text).map(|found| (index, found.start()))
                    })
                    .collect();
                // The cursor resumes at a line start, so a line's hits share one page.
                if hits.len() + line_hits.len() > MAX_HITS_PER_SEARCH {
                    next = Some(SearchCursor {
                        path: path.clone(),
                        offset: line.start,
                    });
                    return Ok(false);
                }
                for (index, start) in line_hits {
                    if let Some(count) = counts.get_mut(index) {
                        *count += 1;
                    }
                    hits.push(json!({
                        "pattern": index, "path": path, "line": line.number, "offset": line.start,
                        "heading": heading, "text": snippet(&text, start),
                    }));
                }
                Ok(true)
            })?;
            if next.is_some() {
                break;
            }
        }
        let exhausted: Vec<usize> = counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count >= MAX_HITS_PER_PATTERN)
            .map(|(index, _)| index)
            .collect();
        Ok(
            json!({"action": "search", "hits": hits, "next": next, "patterns_at_hit_cap": exhausted}),
        )
    }

    fn topic_paths(&self) -> Result<Vec<String>> {
        let excluded = self.store.excluded_topics()?;
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(self.store.scope_dir.join(TOPICS_DIR))? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = format!("{TOPICS_DIR}/{name}");
            if entry.file_type()?.is_file()
                && !name.starts_with('.')
                && name.ends_with(".md")
                && path.len() <= MAX_PATH_BYTES
                && !excluded.contains(&path)
            {
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }
}

fn snippet(line: &str, match_start: usize) -> String {
    let line = line.trim_end();
    if line.len() <= MAX_SNIPPET_BYTES {
        return line.to_owned();
    }
    let start = line.floor_char_boundary(match_start.saturating_sub(MAX_SNIPPET_BYTES / 3));
    let end = line.floor_char_boundary(start + MAX_SNIPPET_BYTES);
    line.get(start..end).unwrap_or_default().to_owned()
}

#[cfg(test)]
#[path = "batch_dream_session_tests.rs"]
mod tests;
