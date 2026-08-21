use crate::model::PiSessionInfo;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use std::{
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

/// Windows filesystems are case-insensitive: fold cwd equality so the pager's
/// cwd matches Pi-recorded casing (drive letters etc.). Unix stays byte-exact.
#[cfg(windows)]
const CWD_COLLATION: &str = " COLLATE NOCASE";
#[cfg(not(windows))]
const CWD_COLLATION: &str = "";

const DEFAULT_PSM_WS_PORT: u16 = 52_131;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
const BUSY_TIMEOUT: Duration = Duration::from_millis(50);

/// PSM is considered available only while its configured local server port is
/// accepting connections. A stale SQLite file is not evidence that PSM runs.
pub fn load_catalog(cwd: &Path, all: bool) -> Option<Vec<PiSessionInfo>> {
    if !psm_server_is_listening(DEFAULT_PSM_WS_PORT) {
        return None;
    }
    load_catalog_from_db(&default_database_path()?, cwd, all).ok()
}

fn default_database_path() -> Option<PathBuf> {
    // std::env::home_dir(): $HOME on Unix, %USERPROFILE% fallback chain on
    // Windows — keeps the PSM index reachable outside macOS/Linux.
    std::env::home_dir().map(|home| {
        home.join(".pi")
            .join("agent")
            .join("sessions")
            .join("sessions.db")
    })
}

fn psm_server_is_listening(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).is_ok()
}

fn load_catalog_from_db(
    db_path: &Path,
    cwd: &Path,
    all: bool,
) -> rusqlite::Result<Vec<PiSessionInfo>> {
    let connection = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA query_only = ON;")?;
    let sql = if all {
        "SELECT s.id, s.path, s.cwd, s.name, s.created, s.modified, s.message_count,
                COALESCE(s.first_message, ''), COALESCE(d.models_json, '[]'),
                COALESCE(d.input_tokens, 0), COALESCE(d.output_tokens, 0),
                COALESCE(d.cache_read_tokens, 0), COALESCE(d.cache_write_tokens, 0),
                d.input_cost + d.output_cost + d.cache_read_cost + d.cache_write_cost,
                s.parent_session_path
           FROM sessions s LEFT JOIN session_details_cache d ON d.path = s.path
          ORDER BY s.modified DESC"
        .to_string()
    } else {
        format!(
            "SELECT s.id, s.path, s.cwd, s.name, s.created, s.modified, s.message_count,
                COALESCE(s.first_message, ''), COALESCE(d.models_json, '[]'),
                COALESCE(d.input_tokens, 0), COALESCE(d.output_tokens, 0),
                COALESCE(d.cache_read_tokens, 0), COALESCE(d.cache_write_tokens, 0),
                d.input_cost + d.output_cost + d.cache_read_cost + d.cache_write_cost,
                s.parent_session_path
           FROM sessions s LEFT JOIN session_details_cache d ON d.path = s.path
          WHERE s.cwd = ?1{CWD_COLLATION} ORDER BY s.modified DESC"
        )
    };
    let mut statement = connection.prepare(&sql)?;
    let rows = if all {
        statement.query_map([], session_from_row)?
    } else {
        statement.query_map(params![cwd.to_string_lossy()], session_from_row)?
    };
    rows.collect()
}

/// A single full-text search hit from PSM's FTS5 index.
#[derive(Debug, Clone)]
pub struct PsmSearchHit {
    pub session_id: String,
    pub cwd: String,
    pub summary: String,
    pub updated_at: String,
    pub score: f32,
    pub matched_fields: Vec<String>,
    pub snippet: Option<String>,
}

/// Search sessions via PSM SQLite — mirrors PSM Rust `full_text_search` +
/// resume-x `searchSessions` (LIKE fallback).
///
/// Order:
/// 1. `message_entries` ⨝ `message_fts` on `rowid` (PSM `search_message_hits_for_mode`)
/// 2. On FTS error / empty → resume-x LIKE on sessions + message_entries
///
/// Returns `None` only when PSM is not listening. Empty query → empty list.
pub fn full_text_search(
    cwd: Option<&Path>,
    query: &str,
    limit: usize,
) -> Option<Vec<PsmSearchHit>> {
    let query = query.trim();
    if query.is_empty() {
        return Some(Vec::new());
    }
    if !psm_server_is_listening(DEFAULT_PSM_WS_PORT) {
        return None;
    }
    let db_path = default_database_path()?;
    full_text_search_db(&db_path, cwd, query, limit).ok()
}

/// One message row for session preview (resume-x `loadSessionMessages` shape).
#[derive(Debug, Clone)]
pub struct PsmPreviewMessage {
    pub role: String,
    pub content: String,
}

/// Load chronological messages for a session preview from PSM SQLite.
/// Matches resume-x `loadSessionMessages` (`message_entries` by `session_path`).
pub fn load_session_messages(session_path: &str, limit: usize) -> Option<Vec<PsmPreviewMessage>> {
    if session_path.trim().is_empty() {
        return Some(Vec::new());
    }
    if !psm_server_is_listening(DEFAULT_PSM_WS_PORT) {
        return None;
    }
    let db_path = default_database_path()?;
    load_session_messages_db(&db_path, session_path, limit).ok()
}

/// Resolve a session's JSONL path from its id when the picker entry lacks path.
pub fn resolve_session_path(session_id: &str) -> Option<String> {
    if session_id.trim().is_empty() {
        return None;
    }
    if !psm_server_is_listening(DEFAULT_PSM_WS_PORT) {
        return None;
    }
    let db_path = default_database_path()?;
    let connection = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    connection.busy_timeout(BUSY_TIMEOUT).ok()?;
    connection
        .query_row(
            "SELECT path FROM sessions WHERE id = ?1 LIMIT 1",
            params![session_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
}

fn load_session_messages_db(
    db_path: &Path,
    session_path: &str,
    limit: usize,
) -> rusqlite::Result<Vec<PsmPreviewMessage>> {
    let connection = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA query_only = ON;")?;
    // resume-x: SELECT role, source_type, content, timestamp FROM message_entries
    let mut stmt = connection.prepare(
        "SELECT role, content FROM message_entries
          WHERE session_path = ?1
          ORDER BY timestamp ASC
          LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![session_path, limit as i64], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (role, content) = row?;
        let body = content.trim();
        if body.is_empty() {
            continue;
        }
        out.push(PsmPreviewMessage {
            role,
            content: body.to_string(),
        });
    }
    Ok(out)
}

fn full_text_search_db(
    db_path: &Path,
    cwd: Option<&Path>,
    query: &str,
    limit: usize,
) -> rusqlite::Result<Vec<PsmSearchHit>> {
    let connection = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA query_only = ON;")?;
    let cwd_val: Option<String> = cwd.map(|c| c.to_string_lossy().to_string());

    // PSM default smart mode for English ≈ MatchMode::Any (OR terms).
    let fts_query = build_fts_query_any(query);
    if !fts_query.is_empty() {
        match run_psm_message_fts(&connection, &fts_query, cwd_val.as_deref(), limit) {
            Ok(hits) if !hits.is_empty() => return Ok(hits),
            Ok(_) => {}
            Err(err) => {
                // Corrupt external-content FTS is common: "missing row N from content table".
                tracing::warn!(error = %err, "PSM message_fts failed; resume-x LIKE fallback");
            }
        }
    }

    // resume-x searchSessions — always works against base tables.
    run_resume_x_like_search(&connection, query, cwd_val.as_deref(), limit)
}

/// PSM `search_message_hits_for_mode` core join:
/// `message_entries m JOIN message_fts ON m.rowid = message_fts.rowid`
/// Content/snippet always come from `message_entries`, never from FTS content=
/// columns (avoids orphan-rowid blowups when only ranking is needed).
fn run_psm_message_fts(
    connection: &Connection,
    fts_query: &str,
    cwd_val: Option<&str>,
    limit: usize,
) -> rusqlite::Result<Vec<PsmSearchHit>> {
    let mut hits: Vec<PsmSearchHit> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // Over-fetch candidates then dedupe per session (PSM uses window functions;
    // we keep one best hit per session_id).
    let fetch = (limit as i64).saturating_mul(4).max(limit as i64);

    let sql = format!(
        "SELECT s.id, s.cwd,
                COALESCE(s.name, s.first_message, ''),
                s.modified,
                -message_fts.rank AS score,
                m.content
           FROM message_entries m
           JOIN message_fts ON m.rowid = message_fts.rowid
           JOIN sessions s ON s.path = m.session_path
          WHERE message_fts MATCH ?1
            AND (?2 IS NULL OR s.cwd = ?2{CWD_COLLATION})
          ORDER BY score DESC, julianday(m.timestamp) DESC
          LIMIT ?3");

    let mut stmt = connection.prepare(&sql)?;
    let rows = stmt.query_map(params![fts_query, cwd_val, fetch], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, f64>(4).unwrap_or(0.0),
            row.get::<_, String>(5)?,
        ))
    })?;

    for row in rows {
        let (id, cwd_str, summary, modified, score, content) = row?;
        if !seen.insert(id.clone()) {
            continue;
        }
        hits.push(PsmSearchHit {
            session_id: id,
            cwd: cwd_str,
            summary: if summary.is_empty() {
                snippet_around(&content, None).unwrap_or_else(|| "(no content)".to_string())
            } else {
                summary
            },
            updated_at: modified,
            score: score as f32,
            matched_fields: vec!["content".to_string()],
            snippet: snippet_around(&content, None),
        });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

/// resume-x `searchSessions` — LIKE on sessions meta + message_entries body.
fn run_resume_x_like_search(
    connection: &Connection,
    query: &str,
    cwd_val: Option<&str>,
    limit: usize,
) -> rusqlite::Result<Vec<PsmSearchHit>> {
    let mut hits: Vec<PsmSearchHit> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let q = format!("%{}%", query.to_lowercase());
    let needle = query.to_lowercase();

    // 1) Session name / first / last message
    {
        let sql = format!(
            "SELECT s.id, s.cwd,
                    COALESCE(NULLIF(s.name, ''), NULLIF(s.first_message, ''),
                             NULLIF(s.last_message, ''), '(no content)'),
                    s.modified,
                    CASE WHEN lower(COALESCE(s.name, '')) LIKE ?1 THEN 'name' ELSE 'message' END
               FROM sessions s
              WHERE (lower(COALESCE(s.name, '')) LIKE ?1
                 OR lower(COALESCE(s.first_message, '')) LIKE ?1
                 OR lower(COALESCE(s.last_message, '')) LIKE ?1)
                AND (?2 IS NULL OR s.cwd = ?2{CWD_COLLATION})
              ORDER BY s.modified DESC
              LIMIT ?3");
        let mut stmt = connection.prepare(&sql)?;
        let rows = stmt.query_map(params![q, cwd_val, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (id, cwd_str, summary, modified, kind) = row?;
            if !seen.insert(id.clone()) {
                continue;
            }
            hits.push(PsmSearchHit {
                session_id: id,
                cwd: cwd_str,
                summary,
                updated_at: modified,
                score: 1.0,
                matched_fields: vec![kind],
                snippet: None,
            });
            if hits.len() >= limit {
                return Ok(hits);
            }
        }
    }

    // 2) Message body
    {
        let remaining = limit - hits.len();
        let fetch = (remaining as i64).saturating_mul(3).max(remaining as i64);
        let sql = format!(
            "SELECT s.id, s.cwd,
                    COALESCE(NULLIF(s.name, ''), '(no content)'),
                    s.modified,
                    me.content
               FROM message_entries me
               JOIN sessions s ON s.path = me.session_path
              WHERE lower(me.content) LIKE ?1
                AND (?2 IS NULL OR s.cwd = ?2{CWD_COLLATION})
              ORDER BY me.timestamp DESC
              LIMIT ?3");
        let mut stmt = connection.prepare(&sql)?;
        let rows = stmt.query_map(params![q, cwd_val, fetch], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (id, cwd_str, summary, modified, content) = row?;
            if !seen.insert(id.clone()) {
                continue;
            }
            let snip = snippet_around(&content, Some(&needle));
            hits.push(PsmSearchHit {
                session_id: id,
                cwd: cwd_str,
                summary: if summary == "(no content)" {
                    snip.clone().unwrap_or(summary)
                } else {
                    summary
                },
                updated_at: modified,
                score: 0.5,
                matched_fields: vec!["content".to_string()],
                snippet: snip,
            });
            if hits.len() >= limit {
                break;
            }
        }
    }

    Ok(hits)
}

/// resume-x snippet: ±40 chars around first match (or head of content).
fn snippet_around(content: &str, needle: Option<&str>) -> Option<String> {
    let flat = content.replace('\n', " ");
    let flat = flat.trim();
    if flat.is_empty() {
        return None;
    }
    if let Some(n) = needle {
        if n.is_empty() {
            return Some(flat.chars().take(80).collect());
        }
        let lower = flat.to_lowercase();
        if let Some(idx) = lower.find(n) {
            let start = idx.saturating_sub(40);
            let end = (idx + n.len() + 40).min(flat.len());
            // Byte-safe: walk char boundaries
            let start = floor_char_boundary(flat, start);
            let end = floor_char_boundary(flat, end);
            let mut snip = flat[start..end].to_string();
            if start > 0 {
                snip = format!("...{snip}");
            }
            if end < flat.len() {
                snip = format!("{snip}...");
            }
            return Some(snip);
        }
    }
    let mut snip: String = flat.chars().take(80).collect();
    if flat.chars().count() > 80 {
        snip.push_str("...");
    }
    Some(snip)
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// PSM MatchMode::Any style: whitespace terms joined with OR.
/// Tokens are FTS-escaped (double quotes doubled). No bogus `"token" *` form.
fn build_fts_query_any(input: &str) -> String {
    let terms: Vec<String> = input
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|token| {
            let escaped = token.replace('"', "\"\"");
            // Bare term; FTS5 unicode61 tokenizes. Quote only if needed for safety.
            if token
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                token.to_string()
            } else {
                format!("\"{escaped}\"")
            }
        })
        .collect();
    terms.join(" OR ")
}

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PiSessionInfo> {
    let models: String = row.get(8)?;
    let model_id = serde_json::from_str::<Value>(&models)
        .ok()
        .and_then(|value| value.as_array()?.last()?.as_str().map(str::to_owned));
    let token_total = [
        row.get::<_, u64>(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
    ]
    .into_iter()
    .sum();
    Ok(PiSessionInfo {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        cwd: row.get(2)?,
        name: row.get(3)?,
        created_at: row.get(4)?,
        modified_at: row.get(5)?,
        message_count: row.get::<_, i64>(6)?.max(0) as usize,
        first_message: row.get(7)?,
        model_id,
        total_tokens: Some(token_total),
        total_cost: row.get(13)?,
        parent_session_path: row.get(14)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "psm-search-{tag}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("sessions.db");
        (dir, db)
    }

    fn seed_psm_style(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, cwd TEXT NOT NULL,
                name TEXT, created TEXT NOT NULL, modified TEXT NOT NULL,
                file_modified TEXT NOT NULL, message_count INTEGER NOT NULL,
                first_message TEXT, user_messages_text TEXT, assistant_messages_text TEXT,
                last_message TEXT, last_message_role TEXT, cached_at TEXT NOT NULL,
                access_count INTEGER DEFAULT 0, last_accessed TEXT,
                parent_session_path TEXT, model TEXT
            );
            CREATE TABLE message_entries (
                id TEXT PRIMARY KEY, entry_id TEXT NOT NULL, session_path TEXT NOT NULL,
                role TEXT NOT NULL, source_type TEXT NOT NULL, content TEXT NOT NULL,
                timestamp TEXT NOT NULL, search_text TEXT NOT NULL DEFAULT '', label TEXT
            );
            CREATE VIRTUAL TABLE message_fts USING fts5(
                session_path UNINDEXED, role UNINDEXED, source_type UNINDEXED, search_text,
                content='message_entries', content_rowid='rowid', tokenize='unicode61'
            );
            INSERT INTO sessions VALUES (
                'sess-1', '/tmp/sess-1.jsonl', '/proj', 'Demo',
                '2026-01-01', '2026-01-02', '2026-01-02', 1,
                'hello world', '', '', '', '',
                '2026-01-02', 0, NULL, NULL, NULL
            );
            INSERT INTO message_entries VALUES (
                'm1', 'e1', '/tmp/sess-1.jsonl', 'user', 'user',
                'hello world about resume picker', '2026-01-01T00:00:00Z',
                'hello world about resume picker', NULL
            );
            INSERT INTO message_fts(message_fts) VALUES('rebuild');
            "#,
        )
        .unwrap();
    }

    fn seed_like_only(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, cwd TEXT NOT NULL,
                name TEXT, created TEXT NOT NULL, modified TEXT NOT NULL,
                file_modified TEXT NOT NULL, message_count INTEGER NOT NULL,
                first_message TEXT, user_messages_text TEXT, assistant_messages_text TEXT,
                last_message TEXT, last_message_role TEXT, cached_at TEXT NOT NULL,
                access_count INTEGER DEFAULT 0, last_accessed TEXT,
                parent_session_path TEXT, model TEXT
            );
            CREATE TABLE message_entries (
                id TEXT PRIMARY KEY, entry_id TEXT NOT NULL, session_path TEXT NOT NULL,
                role TEXT NOT NULL, source_type TEXT NOT NULL, content TEXT NOT NULL,
                timestamp TEXT NOT NULL, search_text TEXT NOT NULL DEFAULT '', label TEXT
            );
            INSERT INTO sessions VALUES (
                'sess-like', '/tmp/like.jsonl', '/proj', 'NamedSession',
                '2026-01-01', '2026-01-02', '2026-01-02', 1,
                'first', '', '', 'last', '',
                '2026-01-02', 0, NULL, NULL, NULL
            );
            INSERT INTO message_entries VALUES (
                'm1', 'e1', '/tmp/like.jsonl', 'user', 'user',
                'unique-widget-xyz in body', '2026-01-01T00:00:00Z',
                'unique-widget-xyz in body', NULL
            );
            "#,
        )
        .unwrap();
    }

    #[test]
    fn fts_query_or_joins_terms() {
        assert_eq!(build_fts_query_any("resume picker"), "resume OR picker");
    }

    #[test]
    fn psm_rowid_join_fts_finds_hits() {
        let (dir, path) = temp_path("fts");
        seed_psm_style(&path);
        let hits = full_text_search_db(&path, None, "resume", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "sess-1");
        assert!(
            hits[0]
                .snippet
                .as_ref()
                .is_some_and(|s| s.contains("resume"))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resume_x_like_when_no_fts_table() {
        let (dir, path) = temp_path("like");
        seed_like_only(&path);
        let hits = full_text_search_db(&path, None, "unique-widget", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "sess-like");
        let title = full_text_search_db(&path, None, "NamedSession", 10).unwrap();
        assert_eq!(title.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_messages_formats_roles() {
        let (dir, path) = temp_path("msg");
        seed_like_only(&path);
        let msgs = load_session_messages_db(&path, "/tmp/like.jsonl", 50).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
        assert!(msgs[0].content.contains("unique-widget"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_db_like_or_fts_smoke() {
        let home = std::env::home_dir().expect("home dir");
        let path = home.join(".pi/agent/sessions/sessions.db");
        if !path.exists() {
            return;
        }
        match full_text_search_db(&path, None, "the", 5) {
            Ok(hits) => {
                eprintln!("live hits: {}", hits.len());
                for h in hits.iter().take(3) {
                    eprintln!("  {} | {:?}", h.session_id, h.snippet);
                }
            }
            Err(e) => panic!("search should not fail hard: {e}"),
        }
    }
}
