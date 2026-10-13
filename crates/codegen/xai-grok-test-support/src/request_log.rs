//! Every route records an entry; the count stays exact after a long test stops retaining entries.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use axum::http::HeaderMap;
use serde_json::Value;
use tokio::sync::watch;

use crate::conversation::ConversationId;
use crate::failure::ObservedFailure;
use crate::inference_request::{
    InferenceEndpoint, InferenceRequest, InferenceRequestKind, first_system_message, offered_tools,
    tool_results,
};

pub(crate) fn authorization_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(String::from)
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    /// The request's place in arrival order over every route, counted from 1, which names the entry
    /// for [`RequestLog::note_failure`] after older entries are evicted.
    sequence: u64,
    pub method: String,
    pub path: String,
    /// Query string without the leading `?`, absent when the request had none.
    pub query: Option<String>,
    pub body: Option<Value>,
    /// Exact body text, set only while request-byte capture is on.
    pub raw_body: Option<String>,
    pub authorization: Option<String>,
    /// Lowercase names in arrival order. Empty for the GET endpoints.
    pub headers: Vec<(String, String)>,
    /// The latency harness builds request timelines from this.
    pub at: std::time::SystemTime,
    /// `None` for every route except the three inference endpoints.
    pub endpoint: Option<InferenceEndpoint>,
    /// Foreground inference requests only.
    pub conversation: Option<usize>,
    /// Scripted turn that served this request, when a conversation script answered it.
    pub scripted_reply: Option<usize>,
    /// What the mock decided to do to an inference request on its own; `None` for a plain answer or a
    /// response the test enqueued. Noted before any hold so a stall still shows it.
    pub observed_failure: Option<ObservedFailure>,
    pub finished: bool,
}

impl LogEntry {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn first_system_prompt(&self) -> Option<String> {
        self.body.as_ref().and_then(first_system_message)
    }

    pub fn offered_tool_names(&self) -> Vec<String> {
        self.body
            .as_ref()
            .map(|body| offered_tools(body).names())
            .unwrap_or_default()
    }

    pub fn tool_results(&self) -> BTreeMap<String, String> {
        self.body.as_ref().map(tool_results).unwrap_or_default()
    }
}

/// An entry holds a whole conversation, so the log evicts oldest first.
const MAX_LOGGED_REQUESTS: usize = 1024;

pub(crate) struct RequestLog {
    count: AtomicU32,
    entries: std::sync::Mutex<Vec<LogEntry>>,
    /// Highest sequence already handed to an observation. `entries` itself stays complete.
    observed_through: AtomicU64,
    keep_entries: AtomicBool,
    /// Foreground inference requests only, so a test can enqueue or cancel once the turn's request is logged.
    inference_arrivals: watch::Sender<usize>,
    capture_request_bytes: AtomicBool,
}

impl RequestLog {
    pub(crate) fn new() -> Self {
        RequestLog {
            count: AtomicU32::new(0),
            entries: std::sync::Mutex::new(Vec::new()),
            observed_through: AtomicU64::new(0),
            keep_entries: AtomicBool::new(true),
            inference_arrivals: watch::Sender::new(0),
            capture_request_bytes: AtomicBool::new(false),
        }
    }

    pub(crate) fn record_get(&self, path: &str) {
        self.record_get_with_authorization(path, None);
    }

    pub(crate) fn record_get_with_authorization(&self, path: &str, authorization: Option<String>) {
        let sequence = self.next_sequence();
        self.keep(LogEntry {
            sequence,
            method: "GET".to_owned(),
            path: path.to_owned(),
            query: None,
            body: None,
            raw_body: None,
            authorization,
            headers: Vec::new(),
            at: std::time::SystemTime::now(),
            endpoint: None,
            conversation: None,
            scripted_reply: None,
            observed_failure: None,
            finished: false,
        });
    }

    pub(crate) fn record(&self, method: &str, path: &str, body: &Value, headers: &HeaderMap) {
        let sequence = self.next_sequence();
        self.keep(RequestLog::entry(sequence, method, path, body, headers));
    }

    /// Returns the sequence that names the entry for [`Self::note_failure`].
    pub(crate) fn record_inference(
        &self,
        request: &InferenceRequest<'_>,
        raw: &[u8],
        query: Option<&str>,
    ) -> u64 {
        let sequence = self.next_sequence();
        let raw_body = self
            .capture_request_bytes
            .load(Ordering::SeqCst)
            .then(|| String::from_utf8_lossy(raw).into_owned());
        self.keep(LogEntry {
            endpoint: Some(request.endpoint()),
            conversation: request.conversation().map(ConversationId::number),
            raw_body,
            query: query.map(str::to_owned),
            ..RequestLog::entry(
                sequence,
                "POST",
                request.endpoint().path(),
                request.body(),
                request.headers(),
            )
        });
        if request.kind() == InferenceRequestKind::Foreground {
            self.inference_arrivals.send_modify(|count| *count += 1);
        }
        sequence
    }

    pub(crate) fn subscribe_inference(&self) -> watch::Receiver<usize> {
        self.inference_arrivals.subscribe()
    }

    pub(crate) fn inference_count(&self) -> usize {
        *self.inference_arrivals.borrow()
    }

    pub(crate) fn note_failure(&self, sequence: u64, failure: ObservedFailure) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.iter_mut().find(|entry| entry.sequence == sequence) {
            entry.observed_failure = Some(failure);
        }
    }

    pub(crate) fn note_finished(&self, sequence: u64) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.iter_mut().find(|entry| entry.sequence == sequence) {
            entry.finished = true;
        }
    }

    fn entry(
        sequence: u64,
        method: &str,
        path: &str,
        body: &Value,
        headers: &HeaderMap,
    ) -> LogEntry {
        LogEntry {
            sequence,
            method: method.to_owned(),
            path: path.to_owned(),
            query: None,
            body: Some(body.clone()),
            raw_body: None,
            authorization: authorization_header(headers),
            headers: headers
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_owned(),
                        String::from_utf8_lossy(value.as_bytes()).into_owned(),
                    )
                })
                .collect(),
            at: std::time::SystemTime::now(),
            endpoint: None,
            conversation: None,
            scripted_reply: None,
            observed_failure: None,
            finished: false,
        }
    }

    pub(crate) fn note_scripted_reply(&self, sequence: u64, reply: usize) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.iter_mut().find(|entry| entry.sequence == sequence) {
            entry.scripted_reply = Some(reply);
        }
    }

    /// Count the arrival; the count stays exact when entries are not kept.
    fn next_sequence(&self) -> u64 {
        u64::from(self.count.fetch_add(1, Ordering::SeqCst)) + 1
    }

    fn keep(&self, entry: LogEntry) {
        if !self.keep_entries.load(Ordering::SeqCst) {
            return;
        }
        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= MAX_LOGGED_REQUESTS {
            entries.remove(0);
        }
        entries.push(entry);
    }

    pub(crate) fn count(&self) -> u32 {
        self.count.load(Ordering::SeqCst)
    }

    /// Foreground inference requests logged for `conversation`, counted from 1 in the scripts.
    pub(crate) fn conversation_requests(&self, conversation: usize) -> usize {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.conversation == Some(conversation))
            .count()
    }

    pub(crate) fn set_keep_entries(&self, enabled: bool) {
        self.keep_entries.store(enabled, Ordering::SeqCst);
    }

    pub(crate) fn set_capture_request_bytes(&self, enabled: bool) {
        self.capture_request_bytes.store(enabled, Ordering::SeqCst);
    }

    pub(crate) fn entries(&self) -> Vec<LogEntry> {
        self.entries.lock().unwrap().clone()
    }

    /// Entries whose sequence is past the previous call. [`Self::entries`] stays the whole log.
    pub(crate) fn take_for_observation(&self) -> Vec<LogEntry> {
        let entries = self.entries.lock().unwrap();
        let seen = self.observed_through.load(Ordering::SeqCst);
        let fresh: Vec<LogEntry> = entries
            .iter()
            .filter(|entry| entry.sequence > seen)
            .cloned()
            .collect();
        if let Some(last) = entries.iter().map(|entry| entry.sequence).max() {
            self.observed_through.fetch_max(last, Ordering::SeqCst);
        }
        fresh
    }

    pub(crate) fn count_for(&self, path: &str) -> usize {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.path == path)
            .count()
    }

    pub(crate) fn has_path_containing(&self, fragment: &str) -> bool {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry.path.contains(fragment))
    }

    pub(crate) fn bodies(&self) -> Vec<Value> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter_map(|entry| entry.body.clone())
            .collect()
    }

    pub(crate) fn summary(&self) -> String {
        let entries = self.entries.lock().unwrap();
        if entries.is_empty() {
            return "(no requests received)".to_owned();
        }
        entries
            .iter()
            .enumerate()
            .map(|(index, entry)| format!("  [{index}] {} {}", entry.method, entry.path))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn last_system_prompt(&self) -> Option<String> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|entry| {
                entry.path.contains("chat/completions") || entry.path.contains("responses")
            })
            .and_then(LogEntry::first_system_prompt)
    }
}
