//! An edit is accepted only against text the model read and the file must
//! still hold those bytes; failed edits defer only their own notes, so one bad
//! edit never holds back the rest of the batch.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::batch_dream::{
    BatchCommit, BatchDreamError, FileChange, FileDependency, MAX_REASON_BYTES, NoteDisposition,
    NoteOutcome, Result, Splice,
};
use crate::batch_dream_io::hash_file;
use crate::batch_dream_outline::{description_range, outline};
use crate::batch_dream_session::{BatchAction, BatchDreamSession, ReadRecord};

pub const MAX_PLAN_EDITS: usize = 32;
pub const MAX_PLAN_TEXT_BYTES: usize = 128 * 1024;
const MAX_CREATE_BYTES: usize = 64 * 1024;
const MAX_ID_BYTES: usize = 32;
/// Section lookups scan the whole outline, unlike the capped one shown to the model.
const MAX_CHECK_SECTIONS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchResponse {
    pub actions: Option<Vec<BatchAction>>,
    pub plan: Option<BatchPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchPlan {
    pub edits: Vec<PlanEdit>,
    pub outcomes: Vec<PlanOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlanEdit {
    Patch {
        id: String,
        read: String,
        old_text: String,
        new_text: String,
    },
    Insert {
        id: String,
        read: String,
        heading: String,
        text: String,
    },
    ReplaceSection {
        id: String,
        read: String,
        heading: String,
        text: String,
    },
    Create {
        id: String,
        path: String,
        content: String,
    },
    UpdateDescription {
        id: String,
        read: String,
        description: String,
    },
}

impl PlanEdit {
    fn id(&self) -> &str {
        match self {
            PlanEdit::Patch { id, .. }
            | PlanEdit::Insert { id, .. }
            | PlanEdit::ReplaceSection { id, .. }
            | PlanEdit::Create { id, .. }
            | PlanEdit::UpdateDescription { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlanOutcome {
    Applied { note: String, edits: Vec<String> },
    NoChange { note: String, evidence: Vec<String> },
    Deferred { note: String, reason: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanCheck {
    pub commit: BatchCommit,
    pub errors: Vec<String>,
    pub stale_paths: BTreeSet<String>,
}

impl PlanCheck {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

enum Resolved {
    Splice {
        path: String,
        base_hash: String,
        splice: Splice,
    },
    Create {
        path: String,
        content: String,
    },
}

impl Resolved {
    fn path(&self) -> &str {
        match self {
            Resolved::Splice { path, .. } | Resolved::Create { path, .. } => path,
        }
    }

    fn text_bytes(&self) -> usize {
        match self {
            Resolved::Splice { splice, .. } => splice.text.len(),
            Resolved::Create { content, .. } => content.len(),
        }
    }
}

struct EditFailure {
    message: String,
    stale_path: Option<String>,
}

impl EditFailure {
    fn new(message: impl Into<String>) -> EditFailure {
        EditFailure {
            message: message.into(),
            stale_path: None,
        }
    }
}

impl From<BatchDreamError> for EditFailure {
    fn from(error: BatchDreamError) -> EditFailure {
        EditFailure::new(error.to_string())
    }
}

impl BatchDreamSession {
    pub fn check(&self, plan: &BatchPlan) -> Result<PlanCheck> {
        let mut check = PlanCheck::default();
        let mut current_hashes: BTreeMap<String, String> = BTreeMap::new();
        let mut resolved: BTreeMap<String, Resolved> = BTreeMap::new();
        let mut failed: BTreeMap<String, String> = BTreeMap::new();
        for (index, edit) in plan.edits.iter().enumerate() {
            let id = edit.id().to_owned();
            let result = if id.is_empty() || id.len() > MAX_ID_BYTES {
                Err(EditFailure::new(format!(
                    "edit id must be 1..={MAX_ID_BYTES} bytes"
                )))
            } else if resolved.contains_key(&id) || failed.contains_key(&id) {
                Err(EditFailure::new(format!("edit id {id} is repeated")))
            } else if index >= MAX_PLAN_EDITS {
                Err(EditFailure::new(format!(
                    "a plan allows {MAX_PLAN_EDITS} edits"
                )))
            } else {
                self.resolve(edit, &mut current_hashes)
            };
            match result {
                Ok(edit) => {
                    resolved.insert(id, edit);
                }
                Err(failure) => {
                    check.stale_paths.extend(failure.stale_path);
                    check.errors.push(format!("{id}: {}", failure.message));
                    failed.insert(id, failure.message);
                }
            }
        }
        reject_conflicts(&mut resolved, &mut failed, &mut check.errors);
        let text_bytes: usize = resolved.values().map(Resolved::text_bytes).sum();
        if text_bytes > MAX_PLAN_TEXT_BYTES {
            check.errors.push(format!(
                "edits add {text_bytes} bytes; a plan allows {MAX_PLAN_TEXT_BYTES}"
            ));
            for id in std::mem::take(&mut resolved).into_keys() {
                failed.insert(id, "plan text budget exceeded".to_owned());
            }
        }
        let used =
            self.settle_outcomes(plan, &resolved, &failed, &mut current_hashes, &mut check)?;
        for id in resolved.keys().filter(|id| !used.contains(*id)) {
            if !failed.contains_key(id) {
                check
                    .errors
                    .push(format!("{id}: no applied outcome uses this edit"));
            }
        }
        let order: BTreeMap<&str, usize> = plan
            .edits
            .iter()
            .enumerate()
            .map(|(index, edit)| (edit.id(), index))
            .collect();
        let mut surviving: Vec<(usize, Resolved)> = resolved
            .into_iter()
            .filter(|(id, _)| used.contains(id))
            .map(|(id, edit)| (order.get(id.as_str()).copied().unwrap_or(usize::MAX), edit))
            .collect();
        surviving.sort_by_key(|(index, _)| *index);
        check.commit.changes = build_changes(surviving.into_iter().map(|(_, edit)| edit));
        Ok(check)
    }

    fn settle_outcomes(
        &self,
        plan: &BatchPlan,
        resolved: &BTreeMap<String, Resolved>,
        failed: &BTreeMap<String, String>,
        current_hashes: &mut BTreeMap<String, String>,
        check: &mut PlanCheck,
    ) -> Result<BTreeSet<String>> {
        let labels: BTreeMap<String, &str> = self
            .lease
            .notes
            .iter()
            .enumerate()
            .map(|(index, note)| (format!("N{}", index + 1), note.path.as_str()))
            .collect();
        let mut settled: BTreeMap<&str, NoteDisposition> = BTreeMap::new();
        let mut used = BTreeSet::new();
        let mut dependencies = BTreeMap::new();
        for outcome in &plan.outcomes {
            let label = match outcome {
                PlanOutcome::Applied { note, .. }
                | PlanOutcome::NoChange { note, .. }
                | PlanOutcome::Deferred { note, .. } => note,
            };
            let Some(path) = labels.get(label.as_str()).copied() else {
                check
                    .errors
                    .push(format!("{label} is not a note in this batch"));
                continue;
            };
            if settled.contains_key(path) {
                check
                    .errors
                    .push(format!("{label} has more than one outcome"));
                continue;
            }
            let disposition = match outcome {
                PlanOutcome::Applied { edits, .. } => {
                    let problem = if edits.is_empty() {
                        Some("applied needs at least one edit".to_owned())
                    } else {
                        edits.iter().find_map(|id| {
                            match (resolved.contains_key(id), failed.get(id)) {
                                (true, _) => None,
                                (false, Some(reason)) => {
                                    Some(format!("edit {id} failed: {reason}"))
                                }
                                (false, None) => Some(format!("edit {id} does not exist")),
                            }
                        })
                    };
                    if let Some(reason) = problem {
                        check.errors.push(format!("{label}: {reason}"));
                        deferred(&reason)
                    } else {
                        used.extend(edits.iter().cloned());
                        NoteDisposition::Applied
                    }
                }
                PlanOutcome::NoChange { evidence, .. } => {
                    match self.evidence(evidence, current_hashes)? {
                        Ok(files) => {
                            dependencies.extend(files);
                            NoteDisposition::NoChange
                        }
                        Err(failure) => {
                            check.stale_paths.extend(failure.stale_path);
                            check.errors.push(format!("{label}: {}", failure.message));
                            deferred(&failure.message)
                        }
                    }
                }
                PlanOutcome::Deferred { reason, .. } => {
                    if reason.trim().is_empty() || reason.len() > MAX_REASON_BYTES {
                        check.errors.push(format!(
                            "{label}: deferral reason must be 1..={MAX_REASON_BYTES} bytes"
                        ));
                        deferred("the model gave no usable reason")
                    } else {
                        deferred(reason)
                    }
                }
            };
            settled.insert(path, disposition);
        }
        for (label, path) in &labels {
            if !settled.contains_key(path) {
                check.errors.push(format!("{label} has no outcome"));
                settled.insert(path, deferred("the plan gave this note no outcome"));
            }
        }
        check.commit.outcomes = settled
            .into_iter()
            .map(|(path, disposition)| NoteOutcome {
                path: path.to_owned(),
                disposition,
            })
            .collect();
        check.commit.dependencies = dependencies
            .into_iter()
            .map(|(path, content_hash)| FileDependency { path, content_hash })
            .collect();
        Ok(used)
    }

    fn evidence(
        &self,
        labels: &[String],
        current_hashes: &mut BTreeMap<String, String>,
    ) -> Result<std::result::Result<Vec<(String, String)>, EditFailure>> {
        if labels.is_empty() {
            return Ok(Err(EditFailure::new(
                "no_change needs at least one read as evidence",
            )));
        }
        let mut files = Vec::new();
        for label in labels {
            let Some(read) = self.reads.iter().find(|read| read.label == *label) else {
                return Ok(Err(EditFailure::new(format!("unknown read {label}"))));
            };
            if let Err(failure) = self.require_current(read, current_hashes) {
                return match failure {
                    FailureOrFatal::Failure(failure) => Ok(Err(failure)),
                    FailureOrFatal::Fatal(error) => Err(error),
                };
            }
            files.push((read.path.clone(), read.file_hash.clone()));
        }
        Ok(Ok(files))
    }

    fn resolve(
        &self,
        edit: &PlanEdit,
        current_hashes: &mut BTreeMap<String, String>,
    ) -> std::result::Result<Resolved, EditFailure> {
        let read = match edit {
            PlanEdit::Create { path, content, .. } => return self.resolve_create(path, content),
            PlanEdit::Patch { read, .. }
            | PlanEdit::Insert { read, .. }
            | PlanEdit::ReplaceSection { read, .. }
            | PlanEdit::UpdateDescription { read, .. } => self
                .reads
                .iter()
                .find(|record| record.label == *read)
                .ok_or_else(|| EditFailure::new(format!("unknown read {read}")))?,
        };
        self.require_current(read, current_hashes)
            .map_err(|failure| match failure {
                FailureOrFatal::Failure(failure) => failure,
                FailureOrFatal::Fatal(error) => EditFailure::from(error),
            })?;
        let absolute = self.store.scope_dir.join(&read.path);
        let control = &self.store.control;
        let splice = match edit {
            PlanEdit::Patch {
                old_text, new_text, ..
            } => {
                let text = read.text.as_deref().ok_or_else(|| {
                    EditFailure::new(format!(
                        "{} is an outline; patch needs a text read",
                        read.label
                    ))
                })?;
                if old_text.is_empty() || old_text == new_text {
                    return Err(EditFailure::new(
                        "old_text must be non-empty and differ from new_text",
                    ));
                }
                let found: Vec<usize> = text
                    .match_indices(old_text.as_str())
                    .map(|(at, _)| at)
                    .collect();
                let [at] = found.as_slice() else {
                    return Err(EditFailure::new(format!(
                        "old_text occurs {} times in {}; it must occur exactly once",
                        found.len(),
                        read.label
                    )));
                };
                let start = read.start + *at as u64;
                Splice {
                    start,
                    end: start + old_text.len() as u64,
                    text: new_text.clone(),
                }
            }
            PlanEdit::Insert { heading, text, .. }
            | PlanEdit::ReplaceSection { heading, text, .. } => {
                let seen = self.reads.iter().any(|record| {
                    record.path == read.path
                        && record.file_hash == read.file_hash
                        && has_seen_heading(record, heading)
                });
                if !seen {
                    return Err(EditFailure::new(format!(
                        "no read of {} shows the heading line {heading:?}; read_topic the file (the outline lists every heading) or read_range from that section's start, then cite that read",
                        read.path
                    )));
                }
                let (sections, _) = outline(&absolute, control, MAX_CHECK_SECTIONS)?;
                let matches: Vec<_> = sections
                    .iter()
                    .filter(|section| section.heading.trim_end() == heading.trim_end())
                    .collect();
                let [section] = matches.as_slice() else {
                    return Err(EditFailure::new(format!(
                        "heading {heading:?} appears {} times in {}; use patch",
                        matches.len(),
                        read.path
                    )));
                };
                let mut text = text.clone();
                if text.trim().is_empty() {
                    return Err(EditFailure::new("section text is empty"));
                }
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                if matches!(edit, PlanEdit::Insert { .. }) {
                    if section.end > 0 && byte_at(&absolute, section.end - 1)? != Some(b'\n') {
                        text.insert(0, '\n');
                    }
                    let size = std::fs::metadata(&absolute)
                        .map_err(BatchDreamError::from)?
                        .len();
                    if section.end < size && !text.ends_with("\n\n") {
                        text.push('\n');
                    }
                    Splice {
                        start: section.end,
                        end: section.end,
                        text,
                    }
                } else {
                    if !self.covers(&read.path, &read.file_hash, section.start, section.end) {
                        return Err(EditFailure::new(format!(
                            "read all of section {heading:?} (bytes {}..{}) before replacing it",
                            section.start, section.end
                        )));
                    }
                    Splice {
                        start: section.start,
                        end: section.end,
                        text,
                    }
                }
            }
            PlanEdit::UpdateDescription { description, .. } => {
                if description.trim().is_empty()
                    || description.len() > crate::v2::MAX_DESCRIPTION_BYTES
                    || description.contains(['\n', '\r'])
                {
                    return Err(EditFailure::new(format!(
                        "description must be one line of 1..={} bytes",
                        crate::v2::MAX_DESCRIPTION_BYTES
                    )));
                }
                match description_range(&absolute, control)? {
                    Ok((start, end)) => Splice {
                        start,
                        end,
                        text: description.trim().to_owned(),
                    },
                    Err(after_title) => Splice {
                        start: after_title,
                        end: after_title,
                        text: format!("{}\n", description.trim()),
                    },
                }
            }
            PlanEdit::Create { .. } => unreachable!("create returned above"),
        };
        Ok(Resolved::Splice {
            path: read.path.clone(),
            base_hash: read.file_hash.clone(),
            splice,
        })
    }

    fn resolve_create(
        &self,
        path: &str,
        content: &str,
    ) -> std::result::Result<Resolved, EditFailure> {
        let connection = self.store.connection()?;
        let absolute = self.store.validate_topic_path(&connection, path)?;
        if absolute.exists() {
            return Err(EditFailure::new(format!(
                "{path} already exists; edit it instead"
            )));
        }
        if !content.starts_with("# ") || content.len() > MAX_CREATE_BYTES {
            return Err(EditFailure::new(format!(
                "new topic content must start with a '# ' title and be at most {MAX_CREATE_BYTES} bytes"
            )));
        }
        let mut content = content.trim_end().to_owned();
        content.push('\n');
        Ok(Resolved::Create {
            path: path.to_owned(),
            content,
        })
    }

    fn require_current(
        &self,
        read: &ReadRecord,
        current_hashes: &mut BTreeMap<String, String>,
    ) -> std::result::Result<(), FailureOrFatal> {
        let current = match current_hashes.get(&read.path) {
            Some(hash) => hash.clone(),
            None => {
                let absolute = self.store.scope_dir.join(&read.path);
                let hash = match hash_file(&absolute, &self.store.control) {
                    Ok(hash) => hash,
                    Err(error @ (BatchDreamError::Interrupted | BatchDreamError::Database(_))) => {
                        return Err(FailureOrFatal::Fatal(error));
                    }
                    Err(error) => return Err(FailureOrFatal::Failure(error.into())),
                };
                current_hashes.insert(read.path.clone(), hash.clone());
                hash
            }
        };
        if current == read.file_hash {
            Ok(())
        } else {
            Err(FailureOrFatal::Failure(EditFailure {
                message: format!("{} changed after {}; read it again", read.path, read.label),
                stale_path: Some(read.path.clone()),
            }))
        }
    }

    fn covers(&self, path: &str, file_hash: &str, start: u64, end: u64) -> bool {
        let mut ranges: Vec<(u64, u64)> = self
            .reads
            .iter()
            .filter(|read| read.path == path && read.file_hash == file_hash)
            .filter_map(|read| {
                read.text
                    .as_ref()
                    .map(|text| (read.start, read.start + text.len() as u64))
            })
            .collect();
        ranges.sort_unstable();
        let mut reached = start;
        for (range_start, range_end) in ranges {
            if range_start > reached {
                break;
            }
            reached = reached.max(range_end);
        }
        reached >= end
    }
}

enum FailureOrFatal {
    Failure(EditFailure),
    Fatal(BatchDreamError),
}

fn deferred(reason: &str) -> NoteDisposition {
    let end = reason.floor_char_boundary(MAX_REASON_BYTES);
    NoteDisposition::Deferred {
        reason: reason.get(..end).unwrap_or(reason).to_owned(),
    }
}

fn has_seen_heading(read: &ReadRecord, heading: &str) -> bool {
    let heading = heading.trim_end();
    !heading.is_empty()
        && (read
            .sections
            .iter()
            .any(|section| section.heading.trim_end() == heading)
            || read
                .text
                .as_deref()
                .is_some_and(|text| text.lines().any(|line| line.trim_end() == heading)))
}

fn byte_at(path: &std::path::Path, offset: u64) -> std::result::Result<Option<u8>, EditFailure> {
    use std::io::{Read as _, Seek as _};
    let mut file = std::fs::File::open(path).map_err(BatchDreamError::from)?;
    file.seek(std::io::SeekFrom::Start(offset))
        .map_err(BatchDreamError::from)?;
    let mut byte = [0u8; 1];
    let count = file.read(&mut byte).map_err(BatchDreamError::from)?;
    Ok((count == 1).then_some(byte[0]))
}

type EditRanges = Vec<(String, Option<(u64, u64)>)>;

fn reject_conflicts(
    resolved: &mut BTreeMap<String, Resolved>,
    failed: &mut BTreeMap<String, String>,
    errors: &mut Vec<String>,
) {
    let mut by_path: BTreeMap<String, EditRanges> = BTreeMap::new();
    for (id, edit) in resolved.iter() {
        let range = match edit {
            Resolved::Splice { splice, .. } => Some((splice.start, splice.end)),
            Resolved::Create { .. } => None,
        };
        by_path
            .entry(edit.path().to_owned())
            .or_default()
            .push((id.clone(), range));
    }
    let mut conflicting = BTreeSet::new();
    for (path, edits) in &by_path {
        for (index, (id, range)) in edits.iter().enumerate() {
            for (other_id, other) in edits.iter().skip(index + 1) {
                let overlaps = match (range, other) {
                    (Some((start, end)), Some((other_start, other_end))) => {
                        let is_insert = start == end;
                        let is_other_insert = other_start == other_end;
                        match (is_insert, is_other_insert) {
                            (true, true) => false,
                            (true, false) => other_start < start && start < other_end,
                            (false, true) => start < other_start && other_start < end,
                            (false, false) => start < other_end && other_start < end,
                        }
                    }
                    _ => true,
                };
                if overlaps {
                    errors.push(format!("{id} and {other_id} overlap in {path}"));
                    conflicting.insert(id.clone());
                    conflicting.insert(other_id.clone());
                }
            }
        }
    }
    for id in conflicting {
        resolved.remove(&id);
        failed.insert(id, "overlaps another edit".to_owned());
    }
}

fn build_changes(edits: impl Iterator<Item = Resolved>) -> Vec<FileChange> {
    let mut splices: BTreeMap<String, (String, Vec<Splice>)> = BTreeMap::new();
    let mut changes = Vec::new();
    for edit in edits {
        match edit {
            Resolved::Create { path, content } => {
                changes.push(FileChange::Create { path, content })
            }
            Resolved::Splice {
                path,
                base_hash,
                splice,
            } => splices
                .entry(path)
                .or_insert_with(|| (base_hash, Vec::new()))
                .1
                .push(splice),
        }
    }
    for (path, (base_hash, mut list)) in splices {
        // A stable sort keeps plan order among inserts at the same point.
        list.sort_by_key(|splice| (splice.start, splice.end));
        let mut merged: Vec<Splice> = Vec::with_capacity(list.len());
        for splice in list {
            match merged.last_mut() {
                Some(last)
                    if last.start == last.end
                        && splice.start == splice.end
                        && last.start == splice.start =>
                {
                    last.text.push_str(&splice.text);
                }
                _ => merged.push(splice),
            }
        }
        changes.push(FileChange::Splice {
            path,
            base_hash,
            splices: merged,
        });
    }
    changes
}

#[must_use]
pub fn response_schema() -> Value {
    let tag = |value: &str| json!({"type": "string", "enum": [value]});
    let string = json!({"type": "string"});
    let integer = json!({"type": "integer"});
    let labels = json!({"type": "array", "items": string});
    let action = json!({"anyOf": [
        {"type": "object", "properties": {"action": tag("read_topic"), "path": string},
         "required": ["action", "path"], "additionalProperties": false},
        {"type": "object", "properties": {"action": tag("read_range"), "path": string,
            "start": integer, "max_bytes": integer},
         "required": ["action", "path", "start", "max_bytes"], "additionalProperties": false},
        {"type": "object", "properties": {"action": tag("search"),
            "patterns": labels,
            "paths": labels,
            "after": {"type": ["object", "null"],
                "properties": {"path": string, "offset": integer},
                "required": ["path", "offset"], "additionalProperties": false}},
         "required": ["action", "patterns", "paths", "after"], "additionalProperties": false},
        {"type": "object", "properties": {"action": tag("list"), "after": {"type": ["string", "null"]}},
         "required": ["action", "after"], "additionalProperties": false}
    ]});
    let edit = |kind: &str, fields: &[&str]| {
        let mut properties = serde_json::Map::new();
        properties.insert("kind".to_owned(), tag(kind));
        properties.insert("id".to_owned(), string.clone());
        let mut names = vec!["kind", "id"];
        for name in fields {
            properties.insert((*name).to_owned(), string.clone());
            names.push(name);
        }
        json!({"type": "object", "properties": properties, "required": names, "additionalProperties": false})
    };
    let edits = json!({"type": "array", "items": {"anyOf": [
        edit("patch", &["read", "old_text", "new_text"]),
        edit("insert", &["read", "heading", "text"]),
        edit("replace_section", &["read", "heading", "text"]),
        edit("create", &["path", "content"]),
        edit("update_description", &["read", "description"])
    ]}});
    let outcomes = json!({"type": "array", "items": {"anyOf": [
        {"type": "object", "properties": {"outcome": tag("applied"), "note": string, "edits": labels},
         "required": ["outcome", "note", "edits"], "additionalProperties": false},
        {"type": "object", "properties": {"outcome": tag("no_change"), "note": string, "evidence": labels},
         "required": ["outcome", "note", "evidence"], "additionalProperties": false},
        {"type": "object", "properties": {"outcome": tag("deferred"), "note": string, "reason": string},
         "required": ["outcome", "note", "reason"], "additionalProperties": false}
    ]}});
    json!({"type": "object",
        "properties": {
            "actions": {"type": ["array", "null"], "items": action},
            "plan": {"type": ["object", "null"],
                "properties": {"edits": edits, "outcomes": outcomes},
                "required": ["edits", "outcomes"], "additionalProperties": false}},
        "required": ["actions", "plan"], "additionalProperties": false})
}
