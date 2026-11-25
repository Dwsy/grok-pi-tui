/**
 * Cross-platform, lazy incremental SQLite index (`sessions-lite.db`) for Pi
 * users who do not have PSM (`sessions.db`) installed.
 *
 * Design principles (inspired by `pi-session-manager`):
 * - **Strict PSM isolation**: Never creates or writes to `sessions.db`. Only
 *   activated when `sessions.db` is absent.
 * - **Lazy incremental scanning**: Walks `~/.pi/agent/sessions` (or
 *   `$PI_CODING_AGENT_DIR/sessions`) and compares `(path, mtimeMs, size)`
 *   against `sessions-lite.db`. Unchanged files are skipped with zero opens;
 *   deleted files are pruned in the same transaction.
 * - **Bounded head+tail probe**: New/changed `.jsonl` files read only the first
 *   16 KB + last 32 KB during catalog sync so multi-MB sessions index in <1ms.
 * - **On-demand detail hydration**: Exact token counts, user/assistant message
 *   splits, and full model lists are parsed lazily for the currently selected
 *   session and cached in `session_details_cache`.
 * - **Multi-runtime & cross-platform**: Works on macOS, Linux, and Windows with
 *   both `node:sqlite` (Pi runtime) and `bun:sqlite` (tests), with an in-memory
 *   fallback if neither SQLite binding is available.
 */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export interface LiteSqliteStatement {
  all: (...args: unknown[]) => Record<string, unknown>[];
  get: (...args: unknown[]) => Record<string, unknown> | undefined;
  run: (...args: unknown[]) => unknown;
}

export interface LiteSqliteDb {
  exec: (sql: string) => void;
  prepare: (sql: string) => LiteSqliteStatement;
  close: () => void;
}

export interface LiteSessionMeta {
  id: string;
  path: string;
  cwd: string;
  name: string;
  created: string;
  modified: string;
  fileMtimeMs: number;
  fileSize: number;
  messageCount: number;
  firstMessage: string;
  userMessagesText: string;
  assistantMessagesText: string;
  model: string;
}

export interface LiteSessionDetails {
  userMessages: number;
  assistantMessages: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  models: string[];
  messageCount: number;
  name?: string;
  model?: string;
}

const HEAD_PROBE_BYTES = 16 * 1024;
const TAIL_PROBE_BYTES = 32 * 1024;
const MAX_SNIPPET_CHARS = 160;
const MAX_SEARCH_TEXT_CHARS = 2048;
const SYNC_COOLDOWN_MS = 3_000;
const COLD_SYNC_BATCH = 200;
const SKIP_DIR_NAMES = new Set(["datasets", "node_modules", ".git", ".cache"]);

const lastSyncAtByDb = new Map<string, number>();

/** Resolve Pi's sessions directory cross-platform (macOS / Linux / Windows). */
export function defaultPiSessionsDir(): string {
  const customAgentDir = process.env.PI_CODING_AGENT_DIR?.trim();
  if (customAgentDir) {
    return path.join(path.resolve(customAgentDir), "sessions");
  }
  return path.join(os.homedir(), ".pi", "agent", "sessions");
}

/** Default path for the fallback `sessions-lite.db` index. */
export function defaultLiteDbPath(sessionsDir = defaultPiSessionsDir()): string {
  return path.join(sessionsDir, "sessions-lite.db");
}

/** Open a read-write SQLite database across Node (`node:sqlite`) and Bun (`bun:sqlite`). */
export function openReadWriteSqlite(dbPath: string): LiteSqliteDb | null {
  if (!dbPath) return null;
  try {
    const dir = path.dirname(dbPath);
    if (!fs.existsSync(dir)) {
      fs.mkdirSync(dir, { recursive: true });
    }
  } catch {
    return null;
  }

  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const nodeSqlite = require("node:sqlite") as {
      DatabaseSync?: new (p: string) => {
        exec: (sql: string) => void;
        prepare: (sql: string) => LiteSqliteStatement;
        close: () => void;
      };
    };
    if (nodeSqlite?.DatabaseSync) {
      const db = new nodeSqlite.DatabaseSync(dbPath);
      return {
        exec: (sql) => db.exec(sql),
        prepare: (sql) => db.prepare(sql),
        close: () => db.close(),
      };
    }
  } catch {
    // Fall through to bun:sqlite.
  }

  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const bunSqlite = require("bun:sqlite") as {
      Database?: new (p: string, opts?: { create?: boolean }) => {
        exec: (sql: string) => void;
        query: (sql: string) => LiteSqliteStatement;
        close: () => void;
      };
    };
    if (bunSqlite?.Database) {
      const db = new bunSqlite.Database(dbPath, { create: true });
      return {
        exec: (sql) => db.exec(sql),
        prepare: (sql) => db.query(sql),
        close: () => db.close(),
      };
    }
  } catch {
    // Neither SQLite binding is available.
  }
  return null;
}

function isCorruptionError(err: unknown): boolean {
  const msg = (err instanceof Error ? err.message : String(err)).toLowerCase();
  return (
    msg.includes("malformed") ||
    msg.includes("disk image") ||
    msg.includes("not a database") ||
    msg.includes("corrupt")
  );
}

function removeCorruptDbFiles(dbPath: string): void {
  for (const suffix of ["", "-wal", "-shm"]) {
    try {
      const target = `${dbPath}${suffix}`;
      if (fs.existsSync(target)) fs.unlinkSync(target);
    } catch {
      // Ignore cleanup errors.
    }
  }
}

function initLiteSchema(db: LiteSqliteDb): void {
  db.exec(`
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = NORMAL;
    PRAGMA busy_timeout = 5000;
    PRAGMA temp_store = MEMORY;
    PRAGMA journal_size_limit = 6144000;

    CREATE TABLE IF NOT EXISTS sessions (
      path TEXT PRIMARY KEY,
      id TEXT NOT NULL,
      cwd TEXT NOT NULL,
      name TEXT,
      created TEXT NOT NULL,
      modified TEXT NOT NULL,
      file_mtime_ms INTEGER NOT NULL DEFAULT 0,
      file_size INTEGER NOT NULL DEFAULT 0,
      message_count INTEGER NOT NULL DEFAULT 0,
      first_message TEXT DEFAULT '',
      user_messages_text TEXT DEFAULT '',
      assistant_messages_text TEXT DEFAULT '',
      model TEXT DEFAULT ''
    );

    CREATE TABLE IF NOT EXISTS session_details_cache (
      path TEXT PRIMARY KEY,
      file_mtime_ms INTEGER NOT NULL DEFAULT 0,
      file_size INTEGER NOT NULL DEFAULT 0,
      user_messages INTEGER NOT NULL DEFAULT 0,
      assistant_messages INTEGER NOT NULL DEFAULT 0,
      input_tokens INTEGER NOT NULL DEFAULT 0,
      output_tokens INTEGER NOT NULL DEFAULT 0,
      cache_read_tokens INTEGER NOT NULL DEFAULT 0,
      cache_write_tokens INTEGER NOT NULL DEFAULT 0,
      models_json TEXT NOT NULL DEFAULT '[]'
    );

    CREATE INDEX IF NOT EXISTS idx_lite_modified ON sessions(modified DESC);
    CREATE INDEX IF NOT EXISTS idx_lite_cwd ON sessions(cwd);
    CREATE INDEX IF NOT EXISTS idx_lite_id ON sessions(id);
  `);
}

function stripInlineThinking(text: string): string {
  return text.replace(/<think>[\s\S]*?<\/think>/gi, "").trim();
}

function extractTextFromContent(content: unknown): string {
  if (typeof content === "string") {
    return stripInlineThinking(content);
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
    if (
      (type === "" || type === "text" || type === "output_text" || type === "input_text") &&
      typeof item.text === "string"
    ) {
      const cleaned = stripInlineThinking(item.text);
      if (cleaned) parts.push(cleaned);
    }
  }
  return parts.join("\n").trim();
}

function formatModelField(model: unknown, provider: unknown): string {
  const m = typeof model === "string" ? model.trim() : "";
  if (!m) return "";
  const p = typeof provider === "string" ? provider.trim() : "";
  if (p && p !== "unknown" && !m.includes("/")) {
    return `${p}/${m}`;
  }
  return m;
}

function inferIdFromPath(filePath: string): string {
  const base = path.basename(filePath, ".jsonl");
  const idx = base.lastIndexOf("_");
  return idx >= 0 && idx + 1 < base.length ? base.slice(idx + 1) : base;
}

/**
 * Recursively collect `.jsonl` session files with `(path, mtimeMs, size)`.
 * Skips hidden/non-session directories and handles permission errors safely.
 */
export function collectSessionJsonlFiles(
  sessionsDir: string,
): Array<{ path: string; mtimeMs: number; size: number }> {
  const out: Array<{ path: string; mtimeMs: number; size: number }> = [];
  if (!sessionsDir || !fs.existsSync(sessionsDir)) return out;

  const walk = (dir: string, depth: number): void => {
    if (depth > 6) return;
    let entries: fs.Dirent[];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const ent of entries) {
      const fullPath = path.join(dir, ent.name);
      if (ent.isDirectory()) {
        if (SKIP_DIR_NAMES.has(ent.name)) continue;
        walk(fullPath, depth + 1);
      } else if (ent.isFile() && ent.name.endsWith(".jsonl")) {
        try {
          const st = fs.statSync(fullPath);
          if (st.size > 0) {
            out.push({
              path: fullPath,
              mtimeMs: Math.floor(st.mtimeMs),
              size: st.size,
            });
          }
        } catch {
          // File disappeared during walk.
        }
      }
    }
  };

  walk(sessionsDir, 0);
  return out;
}

/**
 * Fast bounded head+tail probe for a Pi session JSONL file.
 * Reads at most 16 KB from the start and 32 KB from the end so multi-MB
 * sessions parse in sub-millisecond time without loading full file contents.
 */
export function parseSessionHeaderFast(
  filePath: string,
  fileMtimeMs: number,
  fileSize: number,
): LiteSessionMeta | null {
  let fd: number;
  try {
    fd = fs.openSync(filePath, "r");
  } catch {
    return null;
  }

  try {
    const headLen = Math.min(fileSize, HEAD_PROBE_BYTES);
    const headBuf = Buffer.allocUnsafe(headLen);
    const headRead = fs.readSync(fd, headBuf, 0, headLen, 0);
    const headText = headBuf.subarray(0, headRead).toString("utf8");
    const isWholeFile = fileSize <= HEAD_PROBE_BYTES;

    let id = "";
    let cwd = "";
    let name = "";
    let created = "";
    let latestTs = "";
    let firstMessage = "";
    let model = "";
    let seenMessages = 0;
    const userSnippets: string[] = [];
    const assistantSnippets: string[] = [];
    let userChars = 0;
    let assistantChars = 0;

    const headLines = headText.split("\n");
    // Drop possibly truncated last line unless we read the entire file.
    const headLimit = isWholeFile ? headLines.length : Math.max(1, headLines.length - 1);

    for (let i = 0; i < headLimit; i++) {
      const raw = headLines[i]!.trim();
      if (!raw || raw[0] !== "{") continue;
      let obj: Record<string, unknown>;
      try {
        obj = JSON.parse(raw) as Record<string, unknown>;
      } catch {
        continue;
      }

      const entryType = typeof obj.type === "string" ? obj.type : "";
      const ts = typeof obj.timestamp === "string" ? obj.timestamp : "";
      if (ts && ts > latestTs) latestTs = ts;

      if (entryType === "session") {
        if (!id && typeof obj.id === "string") id = obj.id.trim();
        if (!cwd && typeof obj.cwd === "string") cwd = obj.cwd.trim();
        if (!created && ts) created = ts;
        if (typeof obj.name === "string" && obj.name.trim()) name = obj.name.trim();
        continue;
      }

      if (entryType === "session_info") {
        if (typeof obj.name === "string" && obj.name.trim()) {
          name = obj.name.replace(/\s+/g, " ").trim();
        }
        continue;
      }

      if (entryType === "model_change") {
        const m = formatModelField(obj.modelId ?? obj.model, obj.provider);
        if (m) model = m;
        continue;
      }

      if (entryType === "message" || !entryType) {
        const msg = (obj.message ?? obj) as {
          role?: unknown;
          content?: unknown;
          model?: unknown;
          provider?: unknown;
          timestamp?: unknown;
        };
        const role = typeof msg.role === "string" ? msg.role.toLowerCase() : "";
        if (role !== "user" && role !== "assistant") continue;
        seenMessages += 1;
        if (role === "assistant") {
          const m = formatModelField(msg.model, msg.provider);
          if (m) model = m;
        }
        const text = extractTextFromContent(msg.content);
        if (!text) continue;
        const compact = text.replace(/\s+/g, " ").trim();
        if (role === "user") {
          if (!firstMessage) firstMessage = compact.slice(0, MAX_SNIPPET_CHARS);
          if (userChars < MAX_SEARCH_TEXT_CHARS) {
            const slice = compact.slice(0, MAX_SEARCH_TEXT_CHARS - userChars);
            userSnippets.push(slice);
            userChars += slice.length + 1;
          }
        } else if (assistantChars < MAX_SEARCH_TEXT_CHARS) {
          const slice = compact.slice(0, MAX_SEARCH_TEXT_CHARS - assistantChars);
          assistantSnippets.push(slice);
          assistantChars += slice.length + 1;
        }
      }
    }

    // Probe tail for latest `session_info` rename, latest model, and latest timestamp.
    if (!isWholeFile && fileSize > HEAD_PROBE_BYTES) {
      const tailLen = Math.min(fileSize - HEAD_PROBE_BYTES, TAIL_PROBE_BYTES);
      const tailOffset = Math.max(0, fileSize - tailLen);
      const tailBuf = Buffer.allocUnsafe(tailLen);
      const tailRead = fs.readSync(fd, tailBuf, 0, tailLen, tailOffset);
      const tailLines = tailBuf.subarray(0, tailRead).toString("utf8").split("\n");
      let tailName = "";
      let tailModel = "";
      for (let i = tailLines.length - 1; i >= 0; i--) {
        const raw = tailLines[i]!.trim();
        if (!raw || raw[0] !== "{") continue;
        let obj: Record<string, unknown>;
        try {
          obj = JSON.parse(raw) as Record<string, unknown>;
        } catch {
          continue;
        }
        const ts = typeof obj.timestamp === "string" ? obj.timestamp : "";
        if (ts && ts > latestTs) latestTs = ts;
        const entryType = typeof obj.type === "string" ? obj.type : "";
        if (!tailName && entryType === "session_info" && typeof obj.name === "string" && obj.name.trim()) {
          tailName = obj.name.replace(/\s+/g, " ").trim();
        }
        if (!tailModel && entryType === "model_change") {
          const m = formatModelField(obj.modelId ?? obj.model, obj.provider);
          if (m) tailModel = m;
        }
        if (!tailModel && (entryType === "message" || !entryType)) {
          const msg = (obj.message ?? obj) as { role?: unknown; model?: unknown; provider?: unknown };
          if (msg.role === "assistant") {
            const m = formatModelField(msg.model, msg.provider);
            if (m) tailModel = m;
          }
        }
        if (tailName && tailModel && latestTs) break;
      }
      if (tailName) name = tailName;
      if (tailModel) model = tailModel;
    }

    const fileIso = new Date(fileMtimeMs).toISOString();
    const resolvedCreated = created || fileIso;
    const resolvedModified = latestTs && latestTs > fileIso ? latestTs : fileIso;
    const resolvedId = id || inferIdFromPath(filePath);
    const estimatedMessages = isWholeFile
      ? seenMessages
      : Math.max(seenMessages, Math.round(fileSize / 2048));

    return {
      id: resolvedId,
      path: filePath,
      cwd,
      name,
      created: resolvedCreated,
      modified: resolvedModified,
      fileMtimeMs,
      fileSize,
      messageCount: estimatedMessages,
      firstMessage,
      userMessagesText: userSnippets.join("\n"),
      assistantMessagesText: assistantSnippets.join("\n"),
      model,
    };
  } catch {
    return null;
  } finally {
    try {
      fs.closeSync(fd);
    } catch {
      // Ignore close errors.
    }
  }
}

/**
 * Full single-session JSONL parser used lazily when the user inspects a
 * session in the right-hand preview pane (populates `session_details_cache`).
 */
export function parseSessionDetailsFull(filePath: string): LiteSessionDetails | null {
  if (!filePath || !fs.existsSync(filePath)) return null;
  let raw: string;
  try {
    raw = fs.readFileSync(filePath, "utf8");
  } catch {
    return null;
  }

  let userMessages = 0;
  let assistantMessages = 0;
  let inputTokens = 0;
  let outputTokens = 0;
  let cacheReadTokens = 0;
  let cacheWriteTokens = 0;
  let latestName: string | undefined;
  let latestModel: string | undefined;
  const models: string[] = [];
  const modelSet = new Set<string>();

  for (const line of raw.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed[0] !== "{") continue;
    let obj: Record<string, unknown>;
    try {
      obj = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      continue;
    }
    const entryType = typeof obj.type === "string" ? obj.type : "";
    if (entryType === "session_info" && typeof obj.name === "string" && obj.name.trim()) {
      latestName = obj.name.replace(/\s+/g, " ").trim();
      continue;
    }
    if (entryType === "model_change") {
      const m = formatModelField(obj.modelId ?? obj.model, obj.provider);
      if (m) {
        latestModel = m;
        if (!modelSet.has(m)) {
          modelSet.add(m);
          models.push(m);
        }
      }
      continue;
    }
    if (entryType === "message" || !entryType) {
      const msg = (obj.message ?? obj) as {
        role?: unknown;
        model?: unknown;
        provider?: unknown;
        usage?: Record<string, unknown>;
      };
      const role = typeof msg.role === "string" ? msg.role.toLowerCase() : "";
      if (role === "user") {
        userMessages += 1;
      } else if (role === "assistant") {
        assistantMessages += 1;
        const m = formatModelField(msg.model, msg.provider);
        if (m) {
          latestModel = m;
          if (!modelSet.has(m)) {
            modelSet.add(m);
            models.push(m);
          }
        }
        const usage = msg.usage;
        if (usage && typeof usage === "object") {
          inputTokens += Number(usage.input ?? usage.input_tokens ?? 0) || 0;
          outputTokens += Number(usage.output ?? usage.output_tokens ?? 0) || 0;
          cacheReadTokens += Number(usage.cacheRead ?? usage.cache_read ?? usage.cache_read_tokens ?? 0) || 0;
          cacheWriteTokens += Number(usage.cacheWrite ?? usage.cache_write ?? usage.cache_write_tokens ?? 0) || 0;
        }
      }
    }
  }

  return {
    userMessages,
    assistantMessages,
    inputTokens,
    outputTokens,
    cacheReadTokens,
    cacheWriteTokens,
    models,
    messageCount: userMessages + assistantMessages,
    name: latestName,
    model: latestModel,
  };
}

/**
 * Incrementally sync `sessions-lite.db` against `.jsonl` files under
 * `sessionsDir`. Only new or modified files (by `mtimeMs` + `size`) are
 * opened, and deleted files are pruned in the same transaction.
 */
export function syncSessionLiteDb(options: {
  sessionsDir?: string;
  liteDbPath?: string;
  force?: boolean;
  /** Prioritize JSONLs matching this project cwd first during cold indexing. */
  preferredCwd?: string;
  /** Optional max files to parse in a single cold-start sync pass (defaults to 200). */
  maxBatch?: number;
} = {}): {
  dbPath: string;
  usedSqlite: boolean;
  scanned: number;
  skipped: number;
  removed: number;
  remaining?: number;
  memoryRecords?: LiteSessionMeta[];
} {
  const sessionsDir = options.sessionsDir ?? defaultPiSessionsDir();
  const dbPath = options.liteDbPath ?? defaultLiteDbPath(sessionsDir);
  const now = Date.now();
  const lastSync = lastSyncAtByDb.get(dbPath) ?? 0;

  if (!options.force && lastSync > 0 && now - lastSync < SYNC_COOLDOWN_MS && fs.existsSync(dbPath)) {
    return { dbPath, usedSqlite: true, scanned: 0, skipped: 0, removed: 0 };
  }

  const diskFiles = collectSessionJsonlFiles(sessionsDir);

  const attemptSync = (): {
    dbPath: string;
    usedSqlite: boolean;
    scanned: number;
    skipped: number;
    removed: number;
    memoryRecords?: LiteSessionMeta[];
  } => {
    const db = openReadWriteSqlite(dbPath);
    if (!db) {
      // Runtime has neither node:sqlite nor bun:sqlite: graceful in-memory fallback.
      const memoryRecords: LiteSessionMeta[] = [];
      for (const f of diskFiles) {
        const meta = parseSessionHeaderFast(f.path, f.mtimeMs, f.size);
        if (meta) memoryRecords.push(meta);
      }
      return {
        dbPath,
        usedSqlite: false,
        scanned: memoryRecords.length,
        skipped: 0,
        removed: 0,
        memoryRecords,
      };
    }

    try {
      initLiteSchema(db);
      const existingRows = db
        .prepare(`SELECT path, file_mtime_ms, file_size FROM sessions`)
        .all();
      const existingByPath = new Map<string, { mtimeMs: number; size: number }>();
      for (const row of existingRows) {
        const p = String(row.path ?? "");
        if (p) {
          existingByPath.set(p, {
            mtimeMs: Number(row.file_mtime_ms ?? 0),
            size: Number(row.file_size ?? 0),
          });
        }
      }

      const diskPathSet = new Set<string>();
      const toParse: Array<{ path: string; mtimeMs: number; size: number }> = [];
      let skipped = 0;

      for (const f of diskFiles) {
        diskPathSet.add(f.path);
        const prev = existingByPath.get(f.path);
        if (prev && prev.mtimeMs === f.mtimeMs && prev.size === f.size) {
          skipped += 1;
          continue;
        }
        toParse.push(f);
      }

      const toDelete: string[] = [];
      for (const p of existingByPath.keys()) {
        if (!diskPathSet.has(p)) {
          toDelete.push(p);
        }
      }

      // Prioritize current-project folder and newest sessions first so cold
      // starts surface relevant sessions immediately even before full indexing.
      if (toParse.length > 1) {
        const cwdSlug = options.preferredCwd
          ? options.preferredCwd.replace(/[\\/]+/g, "-").replace(/^-+|-+$/g, "").toLowerCase()
          : "";
        const cwdBase = options.preferredCwd ? path.basename(options.preferredCwd).toLowerCase() : "";
        toParse.sort((a, b) => {
          if (cwdSlug) {
            const aPath = a.path.toLowerCase();
            const bPath = b.path.toLowerCase();
            const aMatch = aPath.includes(cwdSlug) || (cwdBase.length >= 3 && aPath.includes(cwdBase));
            const bMatch = bPath.includes(cwdSlug) || (cwdBase.length >= 3 && bPath.includes(cwdBase));
            if (aMatch !== bMatch) return aMatch ? -1 : 1;
          }
          return b.mtimeMs - a.mtimeMs;
        });
      }

      const batchLimit = options.maxBatch ?? COLD_SYNC_BATCH;
      const isColdChunked = !options.force && existingByPath.size === 0 && toParse.length > batchLimit;
      const batch = isColdChunked ? toParse.slice(0, batchLimit) : toParse;
      const remaining = toParse.length - batch.length;

      if (batch.length > 0 || toDelete.length > 0) {
        const parsed: LiteSessionMeta[] = [];
        for (const f of batch) {
          const meta = parseSessionHeaderFast(f.path, f.mtimeMs, f.size);
          if (meta) parsed.push(meta);
        }

        const upsertStmt = db.prepare(
          `INSERT INTO sessions (
             id, path, cwd, name, created, modified,
             file_mtime_ms, file_size, message_count,
             first_message, user_messages_text, assistant_messages_text, model
           ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(path) DO UPDATE SET
             id = excluded.id,
             cwd = excluded.cwd,
             name = excluded.name,
             created = excluded.created,
             modified = excluded.modified,
             file_mtime_ms = excluded.file_mtime_ms,
             file_size = excluded.file_size,
             message_count = excluded.message_count,
             first_message = excluded.first_message,
             user_messages_text = excluded.user_messages_text,
             assistant_messages_text = excluded.assistant_messages_text,
             model = excluded.model`,
        );
        const deleteSessionStmt = db.prepare(`DELETE FROM sessions WHERE path = ?`);
        const deleteDetailsStmt = db.prepare(`DELETE FROM session_details_cache WHERE path = ?`);

        db.exec("BEGIN IMMEDIATE");
        try {
          for (const meta of parsed) {
            upsertStmt.run(
              meta.id,
              meta.path,
              meta.cwd,
              meta.name || null,
              meta.created,
              meta.modified,
              meta.fileMtimeMs,
              meta.fileSize,
              meta.messageCount,
              meta.firstMessage,
              meta.userMessagesText,
              meta.assistantMessagesText,
              meta.model,
            );
            // Invalidate stale cached details when the JSONL file changed.
            deleteDetailsStmt.run(meta.path);
          }
          for (const p of toDelete) {
            deleteSessionStmt.run(p);
            deleteDetailsStmt.run(p);
          }
          db.exec("COMMIT");
        } catch (txErr) {
          try {
            db.exec("ROLLBACK");
          } catch {
            // Ignore rollback failure.
          }
          throw txErr;
        }
      }

      if (remaining === 0) {
        lastSyncAtByDb.set(dbPath, Date.now());
      } else {
        // Schedule background completion for the remaining cold-start files.
        setImmediate(() => {
          syncSessionLiteDb({ sessionsDir, liteDbPath: dbPath, force: true });
        });
      }
      return {
        dbPath,
        usedSqlite: true,
        scanned: batch.length,
        skipped,
        removed: toDelete.length,
        remaining,
      };
    } finally {
      db.close();
    }
  };

  try {
    return attemptSync();
  } catch (err) {
    if (isCorruptionError(err)) {
      removeCorruptDbFiles(dbPath);
      lastSyncAtByDb.delete(dbPath);
      try {
        return attemptSync();
      } catch {
        // Fall through to memory fallback.
      }
    }
    const memoryRecords: LiteSessionMeta[] = [];
    for (const f of diskFiles) {
      const meta = parseSessionHeaderFast(f.path, f.mtimeMs, f.size);
      if (meta) memoryRecords.push(meta);
    }
    return {
      dbPath,
      usedSqlite: false,
      scanned: memoryRecords.length,
      skipped: 0,
      removed: 0,
      memoryRecords,
    };
  }
}

/**
 * Lazily hydrate and cache exact token/message/model details for a single
 * session inside `sessions-lite.db`. Never touches PSM's `sessions.db`.
 */
export function hydrateLiteSessionDetails(
  sessionPath: string,
  liteDbPath: string,
): LiteSessionDetails | null {
  if (!sessionPath || !fs.existsSync(sessionPath)) return null;
  let mtimeMs = 0;
  let size = 0;
  try {
    const st = fs.statSync(sessionPath);
    mtimeMs = Math.floor(st.mtimeMs);
    size = st.size;
  } catch {
    return null;
  }

  const db = openReadWriteSqlite(liteDbPath);
  if (!db) {
    return parseSessionDetailsFull(sessionPath);
  }

  try {
    initLiteSchema(db);
    const cached = db
      .prepare(
        `SELECT file_mtime_ms, file_size, user_messages, assistant_messages,
                input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, models_json
         FROM session_details_cache
         WHERE path = ?
         LIMIT 1`,
      )
      .get(sessionPath);

    if (
      cached &&
      Number(cached.file_mtime_ms ?? 0) === mtimeMs &&
      Number(cached.file_size ?? 0) === size
    ) {
      let models: string[] = [];
      try {
        const parsed = JSON.parse(String(cached.models_json ?? "[]")) as unknown;
        if (Array.isArray(parsed)) {
          models = parsed.filter((item): item is string => typeof item === "string");
        }
      } catch {
        models = [];
      }
      const userMessages = Number(cached.user_messages ?? 0);
      const assistantMessages = Number(cached.assistant_messages ?? 0);
      return {
        userMessages,
        assistantMessages,
        inputTokens: Number(cached.input_tokens ?? 0),
        outputTokens: Number(cached.output_tokens ?? 0),
        cacheReadTokens: Number(cached.cache_read_tokens ?? 0),
        cacheWriteTokens: Number(cached.cache_write_tokens ?? 0),
        models,
        messageCount: userMessages + assistantMessages,
        model: models[models.length - 1],
      };
    }

    const details = parseSessionDetailsFull(sessionPath);
    if (!details) return null;

    db.exec("BEGIN IMMEDIATE");
    try {
      db.prepare(
        `INSERT INTO session_details_cache (
           path, file_mtime_ms, file_size,
           user_messages, assistant_messages,
           input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
           models_json
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(path) DO UPDATE SET
           file_mtime_ms = excluded.file_mtime_ms,
           file_size = excluded.file_size,
           user_messages = excluded.user_messages,
           assistant_messages = excluded.assistant_messages,
           input_tokens = excluded.input_tokens,
           output_tokens = excluded.output_tokens,
           cache_read_tokens = excluded.cache_read_tokens,
           cache_write_tokens = excluded.cache_write_tokens,
           models_json = excluded.models_json`,
      ).run(
        sessionPath,
        mtimeMs,
        size,
        details.userMessages,
        details.assistantMessages,
        details.inputTokens,
        details.outputTokens,
        details.cacheReadTokens,
        details.cacheWriteTokens,
        JSON.stringify(details.models),
      );
      db.prepare(
        `UPDATE sessions
         SET message_count = ?,
             model = CASE WHEN ? != '' THEN ? ELSE model END,
             name = CASE WHEN ? != '' THEN ? ELSE name END
         WHERE path = ?`,
      ).run(
        details.messageCount,
        details.model ?? "",
        details.model ?? "",
        details.name ?? "",
        details.name ?? "",
        sessionPath,
      );
      db.exec("COMMIT");
    } catch {
      try {
        db.exec("ROLLBACK");
      } catch {
        // Ignore rollback errors.
      }
    }
    return details;
  } catch {
    return parseSessionDetailsFull(sessionPath);
  } finally {
    db.close();
  }
}

/** Clear sync cooldown state (useful in tests). */
export function resetLiteSyncCooldown(liteDbPath?: string): void {
  if (liteDbPath) {
    lastSyncAtByDb.delete(liteDbPath);
  } else {
    lastSyncAtByDb.clear();
  }
}

/**
 * Non-blocking progressive sync: if `sessions-lite.db` already exists, returns
 * immediately so the caller can render cached rows in 0ms and runs incremental
 * sync on the next tick, invoking `onUpdated` only if files changed.
 */
export function scheduleLiteDbBackgroundSync(options: {
  sessionsDir?: string;
  liteDbPath?: string;
  onUpdated?: (stats: { scanned: number; removed: number }) => void;
} = {}): boolean {
  const sessionsDir = options.sessionsDir ?? defaultPiSessionsDir();
  const dbPath = options.liteDbPath ?? defaultLiteDbPath(sessionsDir);
  if (!fs.existsSync(dbPath)) return false;
  setImmediate(() => {
    const res = syncSessionLiteDb({ sessionsDir, liteDbPath: dbPath });
    if (res.scanned > 0 || res.removed > 0) {
      options.onUpdated?.({ scanned: res.scanned, removed: res.removed });
    }
  });
  return true;
}
