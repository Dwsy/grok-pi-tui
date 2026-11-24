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

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export type SessionScope = "global" | "project";

export interface SessionRecord {
  id: string;
  shortId: string;
  path: string;
  cwd: string;
  project: string;
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
}

type SqliteStatement = {
  all: (...args: unknown[]) => Record<string, unknown>[];
};

type SqliteDb = {
  prepare: (sql: string) => SqliteStatement;
  close: () => void;
};

const DEFAULT_MAX_SESSIONS = 300;
const DEFAULT_MAX_PREVIEW_MESSAGES = 120;

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
  return path.join(os.homedir(), ".pi", "agent", "sessions", "sessions.db");
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
 * Load recent sessions with metadata (created/modified timestamps, message
 * counts, project, id, total tokens, model, and user+assistant search text)
 * from PSM's SQLite index.
 */
export function loadSessionPickerRecords(
  options: { dbPath?: string; limit?: number } = {},
): SessionRecord[] {
  const dbPath = options.dbPath ?? defaultPsmDbPath();
  const limit = options.limit ?? DEFAULT_MAX_SESSIONS;
  const db = openReadOnlySqlite(dbPath);
  if (!db) return [];

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
         LEFT JOIN session_details_cache d ON d.path = s.path
         ORDER BY s.modified DESC
         LIMIT ?`,
      )
      .all(limit);

    return rows.map(mapSessionRow);
  } catch {
    return [];
  } finally {
    db.close();
  }
}

/**
 * Full-text search across the entire PSM SQLite catalog (all historical
 * sessions, not just the top 300 loaded in memory). Queries `message_fts`
 * (FTS5) first for fast full-text hits and supplements with `sessions` LIKE
 * matching (handles CJK substrings, IDs, project paths, and models).
 */
export function searchSessionPickerRecords(options: {
  query: string;
  scope: SessionScope;
  currentCwd: string;
  dbPath?: string;
  limit?: number;
}): SessionSearchHit[] {
  const trimmed = options.query.trim();
  if (!trimmed) return [];
  const dbPath = options.dbPath ?? defaultPsmDbPath();
  const limit = options.limit ?? 200;
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
        const ftsRows = db
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
               COALESCE(d.models_json, '') AS models_json,
               snippet(message_fts, 3, '', '', '…', 14) AS fts_snippet
             FROM message_fts
             JOIN sessions s ON s.path = message_fts.session_path
             LEFT JOIN session_details_cache d ON d.path = s.path
             WHERE message_fts MATCH ?
               AND message_fts.source_type IN ('user', 'assistant')
             GROUP BY s.id
             ORDER BY s.modified DESC
             LIMIT ?`,
          )
          .all(ftsQuery, limit);
        for (const row of ftsRows) {
          const record = mapSessionRow(row);
          if (
            options.scope === "project" &&
            !matchesProjectScope(record.cwd, options.currentCwd)
          ) {
            continue;
          }
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
      const likeRows = db
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
           LEFT JOIN session_details_cache d ON d.path = s.path
           WHERE ${whereClauses.join(" AND ")}
           ORDER BY s.modified DESC
           LIMIT ?`,
        )
        .all(...params);
      for (const row of likeRows) {
        const record = mapSessionRow(row);
        if (byId.has(record.id)) continue;
        if (
          options.scope === "project" &&
          !matchesProjectScope(record.cwd, options.currentCwd)
        ) {
          continue;
        }
        const snippet = buildSnippet(record.searchableMessages, firstTerm);
        byId.set(record.id, {
          record,
          matchSnippet: snippet || undefined,
        });
      }
    }

    return [...byId.values()].sort((a, b) =>
      b.record.modified.localeCompare(a.record.modified),
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
  const norm = (p: string) => path.resolve(p).replace(/[\\/]+$/, "");
  if (!sessionCwd || !activeCwd) return false;
  const a = norm(sessionCwd);
  const b = norm(activeCwd);
  return a === b || a.startsWith(`${b}${path.sep}`);
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
  options: { scope: SessionScope; currentCwd: string; query: string },
): SessionSearchHit[] {
  const scoped =
    options.scope === "project"
      ? records.filter((r) => matchesProjectScope(r.cwd, options.currentCwd))
      : records;

  const trimmed = options.query.trim().toLowerCase();
  if (!trimmed) {
    return scoped.map((record) => ({ record }));
  }

  const terms = trimmed.split(/\s+/).filter(Boolean);
  const scored: Array<{ score: number; index: number; hit: SessionSearchHit }> = [];

  for (let i = 0; i < scoped.length; i++) {
    const record = scoped[i]!;
    const metaHaystack = `${record.id} ${record.title} ${record.project} ${record.cwd} ${record.model}`.toLowerCase();
    const msgHaystack = record.searchableMessages.toLowerCase();

    let score = 0;
    let matchedAll = true;
    let snippetTerm: string | undefined;

    for (const term of terms) {
      const inId = record.id.toLowerCase().includes(term);
      const inTitle = record.title.toLowerCase().includes(term);
      const inMeta = metaHaystack.includes(term);
      const inMsg = msgHaystack.includes(term);

      if (!inMeta && !inMsg) {
        matchedAll = false;
        break;
      }
      if (inId) score += 120;
      if (inTitle) score += 90;
      if (inMeta) score += 40;
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
        hit: { record, matchSnippet },
      });
    }
  }

  // Higher score first; tie-break by original recency order.
  scored.sort((a, b) => b.score - a.score || a.index - b.index);
  return scored.map((entry) => entry.hit);
}
