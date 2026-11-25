/**
 * Read-only session data loader, message filter, and search engine for the
 * `@session` TS picker TUI.
 *
 * Reads PSM's SQLite index (`~/.pi/agent/sessions/sessions.db`) with zero
 * startup JSONL scanning, and lazily falls back to JSONL parsing only for the
 * currently previewed session when `message_entries` has no indexed rows.
 *
 * Tool calls, tool results, and thinking/reasoning blocks are strictly excluded
 * from both search and preview.
 */

import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import {
  defaultLiteDbPath,
  defaultPiSessionsDir,
  hydrateLiteSessionDetails,
  syncSessionLiteDb,
  type LiteSessionMeta,
} from "./session-lite-db.ts";

export type SessionScope = "global" | "project";
export type NameFilter = "all" | "named";
export type SortMode = "threaded" | "recent" | "relevance";

export interface SessionRecord {
  id: string;
  shortId: string;
  path: string;
  cwd: string;
  project: string;
  /** PSM session name from `session_info`; absent for unnamed sessions. */
  name?: string;
  title: string;
  created: string;
  modified: string;
  messageCount: number;
  userMessages: number | null;
  assistantMessages: number | null;
  totalTokens: number;
  model: string;
  /** Pre-joined user + assistant text from SQLite for fast in-memory search. */
  searchableMessages: string;
}

export interface PreviewMessage {
  role: "user" | "assistant";
  content: string;
  timestamp: string;
}

export interface SessionSearchHit {
  record: SessionRecord;
  /** Optional excerpt showing where a message-body match occurred. */
  matchSnippet?: string;
  /** Local match rank; higher is better (used by Relevance sort mode). */
  score?: number;
}

export interface SessionRecordDetails {
  userMessages: number | null;
  assistantMessages: number | null;
  totalTokens: number;
  model?: string;
}

type SqliteStatement = {
  all: (...args: unknown[]) => Record<string, unknown>[];
  get?: (...args: unknown[]) => Record<string, unknown> | undefined;
};

type SqliteDb = {
  prepare: (sql: string) => SqliteStatement;
  close: () => void;
};

const DEFAULT_MAX_PREVIEW_MESSAGES = 120;

/** Process-local overlays for JSONL mutations (SQLite is strictly read-only). */
const deletedSessionPaths = new Set<string>();
const renamedSessionNames = new Map<string, string>();

/** Cached full-catalog loads keyed by DB path + query shape, invalidated by DB/WAL mtime+size. */
interface SessionCatalogCacheEntry {
  fingerprint: string;
  records: SessionRecord[];
}
const sessionCatalogCache = new Map<string, SessionCatalogCacheEntry>();

/** Precomputed lowercase search fields per SessionRecord to keep keystroke filtering fast on 10k+ rows. */
interface SessionSearchIndex {
  idLower: string;
  titleLower: string;
  metaLower: string;
  msgLower: string;
}
const searchIndexCache = new WeakMap<SessionRecord, SessionSearchIndex>();

function compareIsoDesc(a: string, b: string): number {
  return b > a ? 1 : b < a ? -1 : 0;
}

function normalizeDirPath(p: string): string {
  if (!p) return "";
  if (p.startsWith("/") && !p.includes("\\") && !p.includes("/./") && !p.includes("/..")) {
    return p.length > 1 ? p.replace(/\/+$/, "") : p;
  }
  return path.resolve(p).replace(/[\\/]+$/, "");
}

function getDbFingerprint(dbPath: string): string {
  try {
    const st = fs.statSync(dbPath);
    let walPart = "0:0";
    try {
      const wst = fs.statSync(`${dbPath}-wal`);
      walPart = `${wst.size}:${wst.mtimeMs}`;
    } catch {
      // No WAL sidecar.
    }
    return `${st.ino}:${st.size}:${st.mtimeMs}:${walPart}`;
  } catch {
    return "";
  }
}

function applyInMemoryOverlays(records: SessionRecord[]): SessionRecord[] {
  if (deletedSessionPaths.size === 0 && renamedSessionNames.size === 0) {
    return records;
  }
  const out: SessionRecord[] = [];
  for (const record of records) {
    const resolved = path.resolve(record.path);
    if (deletedSessionPaths.has(resolved)) continue;
    const renamed = renamedSessionNames.get(resolved);
    if (renamed && (record.name !== renamed || record.title !== renamed)) {
      out.push({ ...record, name: renamed, title: renamed });
    } else {
      out.push(record);
    }
  }
  return out;
}

function getSearchIndex(record: SessionRecord): SessionSearchIndex {
  let cached = searchIndexCache.get(record);
  if (!cached) {
    const idLower = record.id.toLowerCase();
    const titleLower = record.title.toLowerCase();
    const metaLower = `${idLower} ${titleLower} ${record.project.toLowerCase()} ${record.cwd.toLowerCase()} ${record.model.toLowerCase()}`;
    const msgLower = record.searchableMessages ? record.searchableMessages.toLowerCase() : "";
    cached = { idLower, titleLower, metaLower, msgLower };
    searchIndexCache.set(record, cached);
  }
  return cached;
}

/**
 * Open a SQLite database read-only across both Node (`node:sqlite`, used by
 * Pi) and Bun (`bun:sqlite`, used by unit tests).
 */
export function openReadOnlySqlite(dbPath: string): SqliteDb | null {
  if (!dbPath || !fs.existsSync(dbPath)) return null;
  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const nodeSqlite = require("node:sqlite") as {
      DatabaseSync?: new (p: string, opts?: { readOnly?: boolean }) => SqliteDb;
    };
    if (nodeSqlite?.DatabaseSync) {
      return new nodeSqlite.DatabaseSync(dbPath, { readOnly: true });
    }
  } catch {
    // Fall through to bun:sqlite when running under Bun.
  }
  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const bunSqlite = require("bun:sqlite") as {
      Database?: new (p: string, opts?: { readonly?: boolean }) => {
        query: (sql: string) => SqliteStatement;
        close: () => void;
      };
    };
    if (bunSqlite?.Database) {
      const db = new bunSqlite.Database(dbPath, { readonly: true });
      return {
        prepare: (sql: string) => db.query(sql),
        close: () => db.close(),
      };
    }
  } catch {
    // Neither SQLite binding is available in this runtime.
  }
  return null;
}

export function defaultPsmDbPath(): string {
  return path.join(defaultPiSessionsDir(), "sessions.db");
}

export { defaultLiteDbPath, defaultPiSessionsDir };

interface ResolvedSessionDb {
  dbPath: string;
  isLite: boolean;
  memoryRecords?: LiteSessionMeta[];
}

/**
 * Resolve the active session index database:
 * 1. Explicit `dbPath` that exists -> use as-is (read-only unless it is `sessions-lite.db`).
 * 2. PSM's `sessions.db` exists -> strictly read-only, never touches `sessions-lite.db`.
 * 3. PSM's `sessions.db` absent -> lazily sync and use `sessions-lite.db` (or in-memory fallback).
 */
function resolveActiveSessionDb(options: {
  dbPath?: string;
  sessionsDir?: string;
} = {}): ResolvedSessionDb {
  if (options.dbPath) {
    if (fs.existsSync(options.dbPath)) {
      return {
        dbPath: options.dbPath,
        isLite: path.basename(options.dbPath) === "sessions-lite.db",
      };
    }
    if (!options.sessionsDir) {
      return { dbPath: options.dbPath, isLite: false };
    }
  }
  const sessionsDir = options.sessionsDir ?? defaultPiSessionsDir();
  const psmDb = path.join(sessionsDir, "sessions.db");
  if (fs.existsSync(psmDb)) {
    return { dbPath: psmDb, isLite: false };
  }
  const liteDbPath = options.dbPath ?? defaultLiteDbPath(sessionsDir);
  const synced = syncSessionLiteDb({ sessionsDir, liteDbPath });
  return {
    dbPath: synced.dbPath,
    isLite: true,
    memoryRecords: synced.memoryRecords,
  };
}

function mapLiteMetaToRecord(meta: LiteSessionMeta): SessionRecord {
  return mapSessionRow({
    id: meta.id,
    path: meta.path,
    cwd: meta.cwd,
    name: meta.name,
    created: meta.created,
    modified: meta.modified,
    message_count: meta.messageCount,
    first_message: meta.firstMessage,
    user_messages_text: meta.userMessagesText,
    assistant_messages_text: meta.assistantMessagesText,
    model: meta.model,
  });
}

/** Format a token count in millions (`M`), e.g. `1.24M`, `0.05M`, `0.004M`. */
export function formatTokensM(tokens: number): string {
  if (!Number.isFinite(tokens) || tokens <= 0) return "0.00M";
  const millions = tokens / 1_000_000;
  if (millions >= 10) return `${millions.toFixed(1)}M`;
  if (millions >= 0.01) return `${millions.toFixed(2)}M`;
  return `${millions.toFixed(3)}M`;
}

/** Format an ISO timestamp into compact local `YYYY-MM-DD HH:mm`. */
export function formatTimestamp(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed) return "—";
  const date = new Date(trimmed);
  if (Number.isNaN(date.getTime())) {
    return trimmed.slice(0, 16).replace("T", " ");
  }
  const y = date.getFullYear();
  const mo = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  const h = String(date.getHours()).padStart(2, "0");
  const mi = String(date.getMinutes()).padStart(2, "0");
  return `${y}-${mo}-${d} ${h}:${mi}`;
}

/** Format a compact relative age (e.g. `5m`, `2h`, `3d`) for list rows. */
export function formatRelativeAge(raw: string, nowMs = Date.now()): string {
  const ms = Date.parse(raw);
  if (!Number.isFinite(ms)) return "";
  const diffSec = Math.max(0, Math.floor((nowMs - ms) / 1000));
  if (diffSec < 60) return "now";
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin}m`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr}h`;
  const diffDay = Math.floor(diffHr / 24);
  if (diffDay < 30) return `${diffDay}d`;
  const diffMo = Math.floor(diffDay / 30);
  return `${diffMo}mo`;
}

/** Extract a concise project name from a session working directory. */
export function projectNameFromCwd(cwd: string): string {
  const cleaned = cwd.replace(/[\\/]+$/, "");
  if (!cleaned) return "~";
  const base = path.basename(cleaned);
  return base || cleaned;
}

/** Strip `<think>...</think>` blocks if a model emitted inline thinking tags. */
export function stripThinkingTags(text: string): string {
  return text.replace(/<think>[\s\S]*?<\/think>/gi, "").trim();
}

function parsePrimaryModel(directModel: unknown, modelsJson: unknown): string {
  const direct = String(directModel ?? "").trim();
  if (direct) return direct;
  if (typeof modelsJson === "string" && modelsJson.trim()) {
    try {
      const parsed = JSON.parse(modelsJson) as unknown;
      if (Array.isArray(parsed)) {
        for (let i = parsed.length - 1; i >= 0; i--) {
          const item = parsed[i];
          if (typeof item === "string" && item.trim()) return item.trim();
        }
      }
    } catch {
      // Ignore malformed cache JSON.
    }
  }
  return "—";
}

function mapSessionRow(row: Record<string, unknown>): SessionRecord {
  const id = String(row.id ?? "");
  const sessionPath = String(row.path ?? "");
  const cwd = String(row.cwd ?? "");
  const name = String(row.name ?? "").replace(/\s+/g, " ").trim();
  const firstMsg = stripThinkingTags(String(row.first_message ?? ""))
    .replace(/\s+/g, " ")
    .trim();
  const title = name || firstMsg.slice(0, 72) || id.slice(0, 12);
  const totalTokens =
    Number(row.input_tokens ?? 0) +
    Number(row.output_tokens ?? 0) +
    Number(row.cache_read_tokens ?? 0) +
    Number(row.cache_write_tokens ?? 0);
  const userMessages =
    row.detail_user_messages === null || row.detail_user_messages === undefined
      ? null
      : Number(row.detail_user_messages);
  const assistantMessages =
    row.detail_assistant_messages === null || row.detail_assistant_messages === undefined
      ? null
      : Number(row.detail_assistant_messages);
  const userText = stripThinkingTags(String(row.user_messages_text ?? ""));
  const assistantText = stripThinkingTags(String(row.assistant_messages_text ?? ""));

  return {
    id,
    shortId: id.slice(0, 8),
    path: sessionPath,
    cwd,
    project: projectNameFromCwd(cwd),
    name: name || undefined,
    title,
    created: String(row.created ?? ""),
    modified: String(row.modified ?? ""),
    messageCount: Number(row.message_count ?? 0),
    userMessages,
    assistantMessages,
    totalTokens,
    model: parsePrimaryModel(row.model, row.models_json),
    searchableMessages: `${userText}\n${assistantText}`.trim(),
  };
}

/**
 * Load sessions with metadata from PSM's SQLite index without an artificial
 * 300-row cap. Pass `includeDetails: false` for fast full-catalog loading
 * (skipping the per-row `session_details_cache` join) and hydrate details
 * lazily via `loadSessionRecordDetails`.
 */
export function loadSessionPickerRecords(
  options: {
    dbPath?: string;
    sessionsDir?: string;
    limit?: number;
    skipMissingFiles?: boolean;
    includeDetails?: boolean;
  } = {},
): SessionRecord[] {
  const resolved = resolveActiveSessionDb(options);
  const dbPath = resolved.dbPath;
  const limit =
    typeof options.limit === "number" && Number.isFinite(options.limit) && options.limit > 0
      ? Math.floor(options.limit)
      : undefined;
  const includeDetails = options.includeDetails ?? true;
  const skipMissingFiles = options.skipMissingFiles === true;

  if (resolved.memoryRecords) {
    const mapped = resolved.memoryRecords.map(mapLiteMetaToRecord);
    mapped.sort((a, b) => compareIsoDesc(a.modified, b.modified));
    const sliced = limit ? mapped.slice(0, limit) : mapped;
    const filtered = skipMissingFiles
      ? sliced.filter((record) => fs.existsSync(record.path))
      : sliced;
    return applyInMemoryOverlays(filtered);
  }

  const fingerprint = getDbFingerprint(dbPath);
  const cacheKey = `${dbPath}|${limit ?? "all"}|${includeDetails ? "1" : "0"}|${skipMissingFiles ? "1" : "0"}`;
  if (fingerprint) {
    const cached = sessionCatalogCache.get(cacheKey);
    if (cached && cached.fingerprint === fingerprint) {
      return applyInMemoryOverlays(cached.records);
    }
  }

  const db = openReadOnlySqlite(dbPath);
  if (!db) return [];

  try {
    let rows: Record<string, unknown>[];
    if (!includeDetails) {
      // Sequential table scan without JOIN or index-driven random I/O; sort in JS when unbounded.
      const sql = limit
        ? `SELECT
             id,
             path,
             cwd,
             COALESCE(name, '') AS name,
             COALESCE(created, '') AS created,
             COALESCE(modified, '') AS modified,
             COALESCE(message_count, 0) AS message_count,
             COALESCE(first_message, '') AS first_message,
             COALESCE(model, '') AS model
           FROM sessions
           ORDER BY modified DESC
           LIMIT ?`
        : `SELECT
             id,
             path,
             cwd,
             COALESCE(name, '') AS name,
             COALESCE(created, '') AS created,
             COALESCE(modified, '') AS modified,
             COALESCE(message_count, 0) AS message_count,
             COALESCE(first_message, '') AS first_message,
             COALESCE(model, '') AS model
           FROM sessions`;
      rows = limit ? db.prepare(sql).all(limit) : db.prepare(sql).all();
    } else if (limit) {
      rows = db
        .prepare(
          `SELECT
             s.id,
             s.path,
             s.cwd,
             COALESCE(s.name, '') AS name,
             COALESCE(s.created, '') AS created,
             COALESCE(s.modified, '') AS modified,
             COALESCE(s.message_count, 0) AS message_count,
             COALESCE(s.first_message, '') AS first_message,
             COALESCE(s.user_messages_text, '') AS user_messages_text,
             COALESCE(s.assistant_messages_text, '') AS assistant_messages_text,
             COALESCE(s.model, '') AS model,
             d.user_messages AS detail_user_messages,
             d.assistant_messages AS detail_assistant_messages,
             COALESCE(d.input_tokens, 0) AS input_tokens,
             COALESCE(d.output_tokens, 0) AS output_tokens,
             COALESCE(d.cache_read_tokens, 0) AS cache_read_tokens,
             COALESCE(d.cache_write_tokens, 0) AS cache_write_tokens,
             COALESCE(d.models_json, '') AS models_json
           FROM (
             SELECT * FROM sessions ORDER BY modified DESC LIMIT ?
           ) s
           LEFT JOIN session_details_cache d ON d.path = s.path
           ORDER BY s.modified DESC`,
        )
        .all(limit);
    } else {
      rows = db
        .prepare(
          `SELECT
             s.id,
             s.path,
             s.cwd,
             COALESCE(s.name, '') AS name,
             COALESCE(s.created, '') AS created,
             COALESCE(s.modified, '') AS modified,
             COALESCE(s.message_count, 0) AS message_count,
             COALESCE(s.first_message, '') AS first_message,
             COALESCE(s.user_messages_text, '') AS user_messages_text,
             COALESCE(s.assistant_messages_text, '') AS assistant_messages_text,
             COALESCE(s.model, '') AS model,
             d.user_messages AS detail_user_messages,
             d.assistant_messages AS detail_assistant_messages,
             COALESCE(d.input_tokens, 0) AS input_tokens,
             COALESCE(d.output_tokens, 0) AS output_tokens,
             COALESCE(d.cache_read_tokens, 0) AS cache_read_tokens,
             COALESCE(d.cache_write_tokens, 0) AS cache_write_tokens,
             COALESCE(d.models_json, '') AS models_json
           FROM sessions s
           LEFT JOIN session_details_cache d ON d.path = s.path`,
        )
        .all();
    }

    const mapped = rows.map(mapSessionRow);
    if (!limit) {
      mapped.sort((a, b) => compareIsoDesc(a.modified, b.modified));
    }
    const result = skipMissingFiles
      ? mapped.filter((record) => fs.existsSync(record.path))
      : mapped;
    if (fingerprint) {
      if (sessionCatalogCache.size >= 8) sessionCatalogCache.clear();
      sessionCatalogCache.set(cacheKey, { fingerprint, records: result });
    }
    return applyInMemoryOverlays(result);
  } catch {
    return [];
  } finally {
    db.close();
  }
}

/**
 * Fast O(1) point lookup of token/message/model details for a single session
 * from `session_details_cache` (avoids joining 10k+ rows up front).
 */
export function loadSessionRecordDetails(
  sessionPath: string,
  options: { dbPath?: string; sessionsDir?: string } = {},
): SessionRecordDetails | null {
  if (!sessionPath) return null;
  const resolved = resolveActiveSessionDb(options);
  if (resolved.isLite) {
    const details = hydrateLiteSessionDetails(sessionPath, resolved.dbPath);
    if (!details) return null;
    return {
      userMessages: details.userMessages,
      assistantMessages: details.assistantMessages,
      totalTokens:
        details.inputTokens +
        details.outputTokens +
        details.cacheReadTokens +
        details.cacheWriteTokens,
      ...(details.model ? { model: details.model } : {}),
    };
  }
  const dbPath = resolved.dbPath;
  const db = openReadOnlySqlite(dbPath);
  if (!db) return null;
  try {
    const stmt = db.prepare(
      `SELECT
         user_messages AS detail_user_messages,
         assistant_messages AS detail_assistant_messages,
         COALESCE(input_tokens, 0) AS input_tokens,
         COALESCE(output_tokens, 0) AS output_tokens,
         COALESCE(cache_read_tokens, 0) AS cache_read_tokens,
         COALESCE(cache_write_tokens, 0) AS cache_write_tokens,
         COALESCE(models_json, '') AS models_json
       FROM session_details_cache
       WHERE path = ?
       LIMIT 1`,
    );
    const row = stmt.get ? stmt.get(sessionPath) : stmt.all(sessionPath)[0];
    if (!row) return null;
    const userMessages =
      row.detail_user_messages === null || row.detail_user_messages === undefined
        ? null
        : Number(row.detail_user_messages);
    const assistantMessages =
      row.detail_assistant_messages === null || row.detail_assistant_messages === undefined
        ? null
        : Number(row.detail_assistant_messages);
    const totalTokens =
      Number(row.input_tokens ?? 0) +
      Number(row.output_tokens ?? 0) +
      Number(row.cache_read_tokens ?? 0) +
      Number(row.cache_write_tokens ?? 0);
    const model = parsePrimaryModel("", row.models_json);
    return {
      userMessages,
      assistantMessages,
      totalTokens,
      ...(model && model !== "—" ? { model } : {}),
    };
  } catch {
    return null;
  } finally {
    db.close();
  }
}

/** Direct indexed lookup of a single session by full ID or prefix across all history. */
export function findSessionRecordById(
  id: string,
  options: { dbPath?: string; sessionsDir?: string; skipMissingFiles?: boolean } = {},
): SessionRecord | null {
  const trimmed = id.trim();
  if (!trimmed) return null;
  const resolved = resolveActiveSessionDb(options);
  if (resolved.memoryRecords) {
    const records = applyInMemoryOverlays(resolved.memoryRecords.map(mapLiteMetaToRecord));
    const found = records.find((r) => r.id === trimmed || r.id.startsWith(trimmed));
    if (!found) return null;
    if (options.skipMissingFiles && !fs.existsSync(found.path)) return null;
    return found;
  }
  const dbPath = resolved.dbPath;
  const db = openReadOnlySqlite(dbPath);
  if (!db) return null;
  try {
    const rows = db
      .prepare(
        `SELECT
           s.id,
           s.path,
           s.cwd,
           COALESCE(s.name, '') AS name,
           COALESCE(s.created, '') AS created,
           COALESCE(s.modified, '') AS modified,
           COALESCE(s.message_count, 0) AS message_count,
           COALESCE(s.first_message, '') AS first_message,
           '' AS user_messages_text,
           '' AS assistant_messages_text,
           COALESCE(s.model, '') AS model,
           d.user_messages AS detail_user_messages,
           d.assistant_messages AS detail_assistant_messages,
           COALESCE(d.input_tokens, 0) AS input_tokens,
           COALESCE(d.output_tokens, 0) AS output_tokens,
           COALESCE(d.cache_read_tokens, 0) AS cache_read_tokens,
           COALESCE(d.cache_write_tokens, 0) AS cache_write_tokens,
           COALESCE(d.models_json, '') AS models_json
         FROM sessions s
         LEFT JOIN session_details_cache d ON d.path = s.path
         WHERE s.id = ? OR s.id LIKE ?
         ORDER BY s.modified DESC
         LIMIT 1`,
      )
      .all(trimmed, `${trimmed}%`);
    if (rows.length === 0) return null;
    const [record] = applyInMemoryOverlays([mapSessionRow(rows[0]!)]);
    if (!record) return null;
    if (options.skipMissingFiles && !fs.existsSync(record.path)) return null;
    return record;
  } catch {
    return null;
  } finally {
    db.close();
  }
}

/**
 * Full-text search across the entire PSM SQLite catalog. Queries `message_fts`
 * (FTS5) first for fast full-text hits and supplements with `sessions` LIKE
 * matching (handles CJK substrings, IDs, project paths, and models).
 */
export function searchSessionPickerRecords(options: {
  query: string;
  scope: SessionScope;
  currentCwd: string;
  nameFilter?: NameFilter;
  skipMissingFiles?: boolean;
  includeDetails?: boolean;
  dbPath?: string;
  sessionsDir?: string;
  limit?: number;
}): SessionSearchHit[] {
  const trimmed = options.query.trim();
  if (!trimmed) return [];
  const resolved = resolveActiveSessionDb(options);
  if (resolved.memoryRecords) {
    const records = applyInMemoryOverlays(resolved.memoryRecords.map(mapLiteMetaToRecord));
    return filterSessionRecords(records, options);
  }
  const dbPath = resolved.dbPath;
  const limit = options.limit ?? 200;
  const includeDetails = options.includeDetails ?? true;
  const db = openReadOnlySqlite(dbPath);
  if (!db) return [];

  const terms = trimmed.toLowerCase().split(/\s+/).filter(Boolean);
  const firstTerm = terms[0] ?? trimmed.toLowerCase();
  const byId = new Map<string, SessionSearchHit>();

  try {
    // 1. Fast FTS5 full-text search over `message_fts` (user & assistant messages only).
    const ftsQuery = terms
      .map((t) => t.replace(/"/g, "").trim())
      .filter(Boolean)
      .map((t) => `"${t}"*`)
      .join(" AND ");
    if (ftsQuery) {
      try {
        const ftsSql = includeDetails
          ? `SELECT
               s.id,
               s.path,
               s.cwd,
               COALESCE(s.name, '') AS name,
               COALESCE(s.created, '') AS created,
               COALESCE(s.modified, '') AS modified,
               COALESCE(s.message_count, 0) AS message_count,
               COALESCE(s.first_message, '') AS first_message,
               '' AS user_messages_text,
               '' AS assistant_messages_text,
               COALESCE(s.model, '') AS model,
               d.user_messages AS detail_user_messages,
               d.assistant_messages AS detail_assistant_messages,
               COALESCE(d.input_tokens, 0) AS input_tokens,
               COALESCE(d.output_tokens, 0) AS output_tokens,
               COALESCE(d.cache_read_tokens, 0) AS cache_read_tokens,
               COALESCE(d.cache_write_tokens, 0) AS cache_write_tokens,
               COALESCE(d.models_json, '') AS models_json,
               snippet(message_fts, 3, '', '', '…', 14) AS fts_snippet
             FROM message_fts
             JOIN sessions s ON s.path = message_fts.session_path
             LEFT JOIN session_details_cache d ON d.path = s.path
             WHERE message_fts MATCH ?
               AND message_fts.source_type IN ('user', 'assistant')
             GROUP BY s.id
             ORDER BY s.modified DESC
             LIMIT ?`
          : `SELECT
               s.id,
               s.path,
               s.cwd,
               COALESCE(s.name, '') AS name,
               COALESCE(s.created, '') AS created,
               COALESCE(s.modified, '') AS modified,
               COALESCE(s.message_count, 0) AS message_count,
               COALESCE(s.first_message, '') AS first_message,
               '' AS user_messages_text,
               '' AS assistant_messages_text,
               COALESCE(s.model, '') AS model,
               snippet(message_fts, 3, '', '', '…', 14) AS fts_snippet
             FROM message_fts
             JOIN sessions s ON s.path = message_fts.session_path
             WHERE message_fts MATCH ?
               AND message_fts.source_type IN ('user', 'assistant')
             GROUP BY s.id
             ORDER BY s.modified DESC
             LIMIT ?`;
        const ftsRows = db.prepare(ftsSql).all(ftsQuery, limit);
        for (const row of ftsRows) {
          const [record] = applyInMemoryOverlays([mapSessionRow(row)]);
          if (!record) continue;
          if (!isVisiblePickerRecord(record, options)) continue;
          const rawSnippet = stripThinkingTags(String(row.fts_snippet ?? ""))
            .replace(/\s+/g, " ")
            .trim();
          byId.set(record.id, {
            record,
            matchSnippet: rawSnippet || undefined,
          });
        }
      } catch {
        // DB may lack `message_fts` or query had unsupported FTS5 tokens; fall through.
      }
    }

    // 2. Metadata + substring LIKE search across all sessions (covers CJK substrings,
    //    session IDs, titles, project paths, and models).
    if (byId.size < limit) {
      const whereClauses: string[] = [];
      const params: unknown[] = [];
      for (const term of terms) {
        const like = `%${term}%`;
        whereClauses.push(
          `(LOWER(COALESCE(s.id,'')) LIKE ? OR LOWER(COALESCE(s.name,'')) LIKE ? OR LOWER(COALESCE(s.cwd,'')) LIKE ? OR LOWER(COALESCE(s.model,'')) LIKE ? OR LOWER(COALESCE(s.first_message,'')) LIKE ? OR LOWER(COALESCE(s.user_messages_text,'')) LIKE ? OR LOWER(COALESCE(s.assistant_messages_text,'')) LIKE ?)`,
        );
        params.push(like, like, like, like, like, like, like);
      }
      params.push(limit);
      const likeSql = includeDetails
        ? `SELECT
             s.id,
             s.path,
             s.cwd,
             COALESCE(s.name, '') AS name,
             COALESCE(s.created, '') AS created,
             COALESCE(s.modified, '') AS modified,
             COALESCE(s.message_count, 0) AS message_count,
             COALESCE(s.first_message, '') AS first_message,
             COALESCE(s.user_messages_text, '') AS user_messages_text,
             COALESCE(s.assistant_messages_text, '') AS assistant_messages_text,
             COALESCE(s.model, '') AS model,
             d.user_messages AS detail_user_messages,
             d.assistant_messages AS detail_assistant_messages,
             COALESCE(d.input_tokens, 0) AS input_tokens,
             COALESCE(d.output_tokens, 0) AS output_tokens,
             COALESCE(d.cache_read_tokens, 0) AS cache_read_tokens,
             COALESCE(d.cache_write_tokens, 0) AS cache_write_tokens,
             COALESCE(d.models_json, '') AS models_json
           FROM (
             SELECT * FROM sessions s
             WHERE ${whereClauses.join(" AND ")}
             ORDER BY s.modified DESC
             LIMIT ?
           ) s
           LEFT JOIN session_details_cache d ON d.path = s.path
           ORDER BY s.modified DESC`
        : `SELECT
             s.id,
             s.path,
             s.cwd,
             COALESCE(s.name, '') AS name,
             COALESCE(s.created, '') AS created,
             COALESCE(s.modified, '') AS modified,
             COALESCE(s.message_count, 0) AS message_count,
             COALESCE(s.first_message, '') AS first_message,
             COALESCE(s.user_messages_text, '') AS user_messages_text,
             COALESCE(s.assistant_messages_text, '') AS assistant_messages_text,
             COALESCE(s.model, '') AS model
           FROM sessions s
           WHERE ${whereClauses.join(" AND ")}
           ORDER BY s.modified DESC
           LIMIT ?`;
      const likeRows = db.prepare(likeSql).all(...params);
      for (const row of likeRows) {
        const [record] = applyInMemoryOverlays([mapSessionRow(row)]);
        if (!record) continue;
        if (byId.has(record.id)) continue;
        if (!isVisiblePickerRecord(record, options)) continue;
        const snippet = buildSnippet(record.searchableMessages, firstTerm);
        byId.set(record.id, {
          record,
          matchSnippet: snippet || undefined,
        });
      }
    }

    return [...byId.values()].sort((a, b) =>
      compareIsoDesc(a.record.modified, b.record.modified),
    );
  } catch {
    return [];
  } finally {
    db.close();
  }
}

/**
 * Extract only plain user/assistant text from a JSONL message payload.
 * Tool calls (`toolCall`, `tool_use`), tool results (`toolResult`, `tool_result`),
 * and thinking blocks (`thinking`, `redacted_thinking`) are strictly excluded.
 */
export function extractVisibleMessageText(content: unknown): string {
  if (typeof content === "string") {
    return stripThinkingTags(content);
  }
  if (!Array.isArray(content)) return "";

  const parts: string[] = [];
  for (const block of content) {
    if (!block || typeof block !== "object") continue;
    const item = block as { type?: unknown; text?: unknown };
    const type = typeof item.type === "string" ? item.type.toLowerCase() : "";
    if (
      type === "thinking" ||
      type === "redacted_thinking" ||
      type === "toolcall" ||
      type === "tool_call" ||
      type === "tool_use" ||
      type === "toolresult" ||
      type === "tool_result"
    ) {
      continue;
    }
    if ((type === "" || type === "text" || type === "output_text" || type === "input_text") && typeof item.text === "string") {
      const cleaned = stripThinkingTags(item.text);
      if (cleaned) parts.push(cleaned);
    }
  }
  return parts.join("\n").trim();
}

/**
 * Parse user & assistant text messages directly from a Pi session JSONL file,
 * hiding tool calls, tool results, and thinking blocks.
 */
export function loadPreviewFromJsonl(
  jsonlPath: string,
  maxMessages = DEFAULT_MAX_PREVIEW_MESSAGES,
): PreviewMessage[] {
  if (!jsonlPath || !fs.existsSync(jsonlPath)) return [];
  try {
    const raw = fs.readFileSync(jsonlPath, "utf8");
    const lines = raw.split("\n");
    const messages: PreviewMessage[] = [];
    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      let parsed: Record<string, unknown>;
      try {
        parsed = JSON.parse(trimmed) as Record<string, unknown>;
      } catch {
        continue;
      }
      if (parsed.type && parsed.type !== "message") continue;
      const msg = (parsed.message ?? parsed) as {
        role?: unknown;
        content?: unknown;
        timestamp?: unknown;
      };
      const role = typeof msg.role === "string" ? msg.role.toLowerCase() : "";
      if (role !== "user" && role !== "assistant") continue;
      const content = extractVisibleMessageText(msg.content);
      if (!content) continue;
      const ts =
        typeof parsed.timestamp === "string"
          ? parsed.timestamp
          : typeof msg.timestamp === "string"
            ? msg.timestamp
            : "";
      messages.push({ role, content, timestamp: ts });
    }
    return messages.length > maxMessages ? messages.slice(-maxMessages) : messages;
  } catch {
    return [];
  }
}

/**
 * Load conversation preview for a single session, strictly hiding tool calls
 * and thinking blocks. Queries PSM's `message_entries` first and falls back to
 * JSONL only if no indexed rows exist for the session.
 */
export function loadSessionPreviewMessages(
  sessionPath: string,
  options: { dbPath?: string; maxMessages?: number } = {},
): PreviewMessage[] {
  const maxMessages = options.maxMessages ?? DEFAULT_MAX_PREVIEW_MESSAGES;
  const dbPath = options.dbPath ?? defaultPsmDbPath();
  const db = openReadOnlySqlite(dbPath);
  if (db) {
    try {
      const rows = db
        .prepare(
          `SELECT role, content, timestamp
           FROM message_entries
           WHERE session_path = ?
             AND role IN ('user', 'assistant')
             AND source_type IN ('user', 'assistant')
           ORDER BY timestamp ASC
           LIMIT ?`,
        )
        .all(sessionPath, maxMessages);
      if (rows.length > 0) {
        const messages: PreviewMessage[] = [];
        for (const row of rows) {
          const role = row.role === "user" ? "user" : row.role === "assistant" ? "assistant" : null;
          if (!role) continue;
          const content = stripThinkingTags(String(row.content ?? ""));
          if (!content) continue;
          messages.push({
            role,
            content,
            timestamp: String(row.timestamp ?? ""),
          });
        }
        if (messages.length > 0) return messages;
      }
    } catch {
      // Fall back to JSONL below.
    } finally {
      db.close();
    }
  }
  return loadPreviewFromJsonl(sessionPath, maxMessages);
}

/**
 * Check whether a session belongs to the active project directory (exact match
 * or nested subdirectory).
 */
export function matchesProjectScope(sessionCwd: string, activeCwd: string): boolean {
  if (!sessionCwd || !activeCwd) return false;
  const a = normalizeDirPath(sessionCwd);
  const b = normalizeDirPath(activeCwd);
  return a === b || a.startsWith(`${b}${path.sep}`);
}

/** Fast project-scope count without allocating intermediate hit arrays. */
export function countProjectSessions(
  records: readonly SessionRecord[],
  activeCwd: string,
): number {
  if (!activeCwd) return 0;
  const b = normalizeDirPath(activeCwd);
  const bPrefix = `${b}${path.sep}`;
  let count = 0;
  for (let i = 0; i < records.length; i++) {
    const cwd = records[i]!.cwd;
    if (!cwd) continue;
    const a = normalizeDirPath(cwd);
    if (a === b || a.startsWith(bPrefix)) count += 1;
  }
  return count;
}
function matchesPickerFilters(
  record: SessionRecord,
  options: { scope: SessionScope; currentCwd: string; nameFilter?: NameFilter },
): boolean {
  if (options.scope === "project" && !matchesProjectScope(record.cwd, options.currentCwd)) {
    return false;
  }
  return options.nameFilter !== "named" || Boolean(record.name?.trim());
}

function isVisiblePickerRecord(
  record: SessionRecord,
  options: {
    scope: SessionScope;
    currentCwd: string;
    nameFilter?: NameFilter;
    skipMissingFiles?: boolean;
  },
): boolean {
  return (
    matchesPickerFilters(record, options) &&
    (!options.skipMissingFiles || fs.existsSync(record.path))
  );
}

/** Append a Pi/PSM-compatible `session_info` entry. Never touches the SQLite index. */
export function appendSessionNameToJsonl(
  sessionPath: string,
  name: string,
): { ok: true } | { ok: false; error: string } {
  const trimmed = name.replace(/[\r\n]+/g, " ").trim();
  if (!trimmed) return { ok: false, error: "Name is empty" };
  if (!fs.existsSync(sessionPath)) return { ok: false, error: "Session file not found" };
  const entry = {
    type: "session_info",
    id: `session-info-${Date.now()}-${crypto.randomBytes(4).toString("hex")}`,
    parentId: null,
    timestamp: new Date().toISOString(),
    name: trimmed,
  };
  try {
    let needsLeadingNewline = false;
    const stat = fs.statSync(sessionPath);
    if (stat.size > 0) {
      const fd = fs.openSync(sessionPath, "r");
      try {
        const tail = Buffer.alloc(1);
        const bytesRead = fs.readSync(fd, tail, 0, 1, stat.size - 1);
        needsLeadingNewline = bytesRead === 1 && tail[0] !== 0x0a;
      } finally {
        fs.closeSync(fd);
      }
    }
    const prefix = needsLeadingNewline ? "\n" : "";
    fs.appendFileSync(sessionPath, `${prefix}${JSON.stringify(entry)}\n`, "utf8");
    renamedSessionNames.set(path.resolve(sessionPath), trimmed);
    return { ok: true };
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}

/** Move a session JSONL to trash, falling back to unlink. Never touches the SQLite index. */
export function deleteSessionJsonl(
  sessionPath: string,
): { ok: true; method: "trash" | "unlink" | "missing" } | { ok: false; error: string } {
  if (!sessionPath.trim()) return { ok: false, error: "Session path is empty" };
  if (!fs.existsSync(sessionPath)) return { ok: true, method: "missing" };

  const args = sessionPath.startsWith("-") ? ["--", sessionPath] : [sessionPath];
  try {
    const result = spawnSync("trash", args, { encoding: "utf8" });
    if (result.status === 0 || !fs.existsSync(sessionPath)) {
      deletedSessionPaths.add(path.resolve(sessionPath));
      return { ok: true, method: "trash" };
    }
  } catch {
    // `trash` is optional; fall through to permanent deletion.
  }
  try {
    fs.unlinkSync(sessionPath);
    deletedSessionPaths.add(path.resolve(sessionPath));
    return { ok: true, method: "unlink" };
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}

function buildSnippet(text: string, term: string, radius = 36): string {
  const lower = text.toLowerCase();
  const idx = lower.indexOf(term);
  if (idx < 0) return "";
  const start = Math.max(0, idx - radius);
  const end = Math.min(text.length, idx + term.length + radius);
  const slice = text.slice(start, end).replace(/\s+/g, " ").trim();
  return `${start > 0 ? "…" : ""}${slice}${end < text.length ? "…" : ""}`;
}

/**
 * Filter and rank sessions by scope (`global` vs `project`) and multi-term
 * search query across session metadata plus user & assistant messages only.
 */
export function filterSessionRecords(
  records: readonly SessionRecord[],
  options: {
    scope: SessionScope;
    currentCwd: string;
    query: string;
    nameFilter?: NameFilter;
  },
): SessionSearchHit[] {
  const isProjectScope = options.scope === "project";
  const namedOnly = options.nameFilter === "named";
  const normActiveCwd = isProjectScope && options.currentCwd ? normalizeDirPath(options.currentCwd) : "";
  const activePrefix = normActiveCwd ? `${normActiveCwd}${path.sep}` : "";

  const scoped: SessionRecord[] = [];
  for (let i = 0; i < records.length; i++) {
    const record = records[i]!;
    if (isProjectScope) {
      if (!normActiveCwd || !record.cwd) continue;
      const a = normalizeDirPath(record.cwd);
      if (a !== normActiveCwd && !a.startsWith(activePrefix)) continue;
    }
    if (namedOnly && !record.name?.trim()) continue;
    scoped.push(record);
  }

  const trimmed = options.query.trim().toLowerCase();
  if (!trimmed) {
    return scoped.map((record) => ({ record }));
  }

  const terms = trimmed.split(/\s+/).filter(Boolean);
  const scored: Array<{ score: number; index: number; hit: SessionSearchHit }> = [];

  for (let i = 0; i < scoped.length; i++) {
    const record = scoped[i]!;
    const idx = getSearchIndex(record);

    let score = 0;
    let matchedAll = true;
    let snippetTerm: string | undefined;

    for (const term of terms) {
      const inMeta = idx.metaLower.includes(term);
      const inMsg = idx.msgLower ? idx.msgLower.includes(term) : false;

      if (!inMeta && !inMsg) {
        matchedAll = false;
        break;
      }
      if (inMeta) {
        score += 40;
        if (idx.idLower.includes(term)) score += 120;
        if (idx.titleLower.includes(term)) score += 90;
      }
      if (inMsg) {
        score += 60;
        snippetTerm ??= term;
      }
    }

    if (matchedAll) {
      const matchSnippet = snippetTerm
        ? buildSnippet(record.searchableMessages, snippetTerm)
        : undefined;
      scored.push({
        score,
        index: i,
        hit: { record, matchSnippet, score },
      });
    }
  }

  // Higher score first; tie-break by original recency order.
  scored.sort((a, b) => b.score - a.score || a.index - b.index);
  return scored.map((entry) => entry.hit);
}
