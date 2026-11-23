/**
 * grok-pi & Pi-native extensible `@` object mentions (`@skill`, `@session`, `@mcp`, `@file`).
 *
 * Design & performance principles:
 * - **Files (`@file` / `@f` / `@path` / `@文件`)**: handled natively by high-performance
 *   Rust (`FuzzyFileMatcherDaemon` / `nucleo` in `xai-grok-pager`, just like `/`).
 * - **Skills (`@skill`)**: sourced directly from Pi's own live skill registry
 *   (`pi.getCommands()` + `event.systemPromptOptions.skills`) — zero directory scanning.
 * - **MCP (`@mcp`)**: sourced directly from Pi's own live MCP/tool registry
 *   (`pi.getMcpServers()` + `pi.getAllTools()`) — zero `mcp.json` disk reads.
 * - **Sessions (`@session`)**: queried read-only from PSM's SQLite index (`sessions.db`)
 *   with zero JSONL disk scanning and TTL caching; if `node:sqlite` is unavailable,
 *   `pi-grok-adapter` hydrates it via compiled `rusqlite`.
 * - **Standalone Pi compatible**: `before_agent_start` resolution works in both `grok-pi`
 *   and standalone `pi`; only the private `__pi_grok_mentions__` status push is gated
 *   on `PI_GROK === "1"`, and payloads are diff-checked before publishing.
 */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import {
  formatTokensM,
  loadSessionPickerRecords,
  loadSessionPreviewMessages,
  type SessionRecord,
} from "./session-data.ts";
import { createSessionPickerTui, type SessionPickerTheme } from "./session-picker-tui.ts";
import { buildSkillRecords, type SkillRecord } from "./skill-data.ts";
import { createSkillPickerTui } from "./skill-picker-tui.ts";
import { createFilePickerTui, type FilePickerSelection } from "./file-picker-tui.ts";
import {
  createMcpPickerTui,
  type McpServerRecord,
  type McpToolRecord,
} from "./mcp-picker-tui.ts";

/** Must match `EXTENSION_MENTIONS_STATUS_KEY` in pi-grok-adapter. */
const STATUS_KEY = "__pi_grok_mentions__";
/** Must match `EXTENSION_MENTION_INSERT_STATUS_KEY` in pi-grok-adapter. */
const INSERT_STATUS_KEY = "__pi_grok_mention_insert__";
const SESSION_PICKER_COMMAND = "__pi_mention_session_picker";
const SKILL_PICKER_COMMAND = "__pi_mention_skill_picker";
const FILE_PICKER_COMMAND = "__pi_mention_file_picker";
const MCP_PICKER_COMMAND = "__pi_mention_mcp_picker";
const RESUME_SESSION_COMMAND = "__pi_resume_session";
const MAX_THREADS = 50;
const MAX_CATALOG_BYTES = 450 * 1024;
const THREAD_CACHE_TTL_MS = 5_000;

/**
 * Cap on entries listed for an `@folder` reference. A folder reference exists
 * to tell the model what is inside; dumping thousands of paths would blow the
 * context for no gain, and the model can always read further with its tools.
 */
const MAX_DIR_ENTRIES = 200;

export interface MentionItem {
  id: string;
  title: string;
  subtitle: string;
  uri: string;
  insertText: string;
  searchText?: string;
  /** Optional local path (e.g. SKILL.md or session JSONL) used for prompt resolution. */
  sourcePath?: string;
}

export interface MentionProviderAction {
  /** Pi KeyId format (e.g. `"ctrl+o"`). */
  key: string;
  /** Short label shown in the bottom shortcuts bar while this provider is active. */
  label: string;
  /** Hidden Pi slash command invoked via RPC when the shortcut is pressed. */
  command: string;
}

export interface MentionProviderSpec {
  kind: string;
  aliases: string[];
  label: string;
  description: string;
  /** Optional contextual shortcut action shown in the bottom bar when this provider is active. */
  action?: MentionProviderAction;
  /** Dynamic item collector invoked on session lifecycle / explicit refresh. */
  collect: (ctx: { cwd: string }) => MentionItem[];
  /** Optional custom resolver for `before_agent_start` context injection. */
  resolve?: (id: string, ctx: { cwd: string }) => string | null;
}

type SerializedProvider = {
  kind: string;
  aliases: string[];
  label: string;
  description: string;
  action?: MentionProviderAction;
  items: Omit<MentionItem, "sourcePath">[];
};

type PatchableUi = {
  setStatus?: (key: string, text?: string) => void;
};

type GlobalWithMentions = typeof globalThis & {
  __piGrokRegisterMentionProvider?: (provider: MentionProviderSpec) => void;
  __piGrokRefreshMentions?: () => void;
};

type PiSkillMeta = {
  name?: string;
  description?: string;
  filePath?: string;
  disableModelInvocation?: boolean;
};

type PiCommandMeta = {
  name?: string;
  description?: string;
  source?: string;
  sourceInfo?: { path?: string };
};

type PiMcpServerMeta = {
  name?: string;
  config?: { url?: string; command?: string };
};

type PiToolMeta = {
  name?: string;
  description?: string;
  sourceInfo?: { source?: string; scope?: string; path?: string };
};

const customProviders = new Map<string, MentionProviderSpec>();
/** Live skill cache populated from Pi's own `systemPromptOptions.skills` & `pi.getCommands()`. */
const cachedPiSkills = new Map<string, MentionItem>();
let cachedThreads: MentionItem[] = [];
let cachedThreadsAt = 0;
let lastUi: PatchableUi | undefined;
let lastCwd = process.cwd();
let lastPublishedPayload = "";

function isGrokPiHost(): boolean {
  return process.env.PI_GROK === "1";
}

function upsertSkill(id: string, description?: string, filePath?: string): void {
  const cleanId = id.trim();
  if (!cleanId) return;
  const desc = String(description ?? "").replace(/\s+/g, " ").trim().slice(0, 96);
  const existing = cachedPiSkills.get(cleanId);
  const resolvedPath = filePath || existing?.sourcePath;
  const resolvedSubtitle = desc || existing?.subtitle || "";
  cachedPiSkills.set(cleanId, {
    id: cleanId,
    title: cleanId,
    subtitle: resolvedSubtitle,
    uri: `grok-pi://skill/${cleanId}`,
    insertText: `@skill:${cleanId}`,
    searchText: `${cleanId} ${resolvedSubtitle}`.trim(),
    sourcePath: resolvedPath,
  });
}

/**
 * Collect skills exclusively from Pi's own registered skill catalog
 * (`pi.getCommands()` + cached `event.systemPromptOptions.skills`).
 * Zero filesystem scanning.
 */
function collectSkillsFromPi(pi: ExtensionAPI): MentionItem[] {
  try {
    const commands = (pi.getCommands?.() ?? []) as PiCommandMeta[];
    for (const cmd of commands) {
      const rawName = String(cmd.name ?? "");
      const isSkill = cmd.source === "skill" || rawName.startsWith("skill:");
      if (!isSkill) continue;
      const id = rawName.replace(/^skill:/, "");
      upsertSkill(id, cmd.description, cmd.sourceInfo?.path);
    }
  } catch {
    // Pi command registry not hydrated yet.
  }

  return [...cachedPiSkills.values()].sort((a, b) => a.id.localeCompare(b.id));
}

/**
 * Query PSM's SQLite index directly (read-only, zero disk scan of JSONL files)
 * with a short in-memory TTL cache so repeated lookups stay O(1).
 * If Node's built-in `node:sqlite` is unavailable, returns `[]` so `pi-grok-adapter`
 * transparently hydrates thread items using its compiled `rusqlite` PSM reader.
 */
function collectThreadsFromPsmDb(force = false): MentionItem[] {
  const now = Date.now();
  if (!force && cachedThreadsAt > 0 && now - cachedThreadsAt < THREAD_CACHE_TTL_MS) {
    return cachedThreads;
  }

  const dbPath = path.join(os.homedir(), ".pi", "agent", "sessions", "sessions.db");
  if (!fs.existsSync(dbPath)) {
    cachedThreads = [];
    cachedThreadsAt = now;
    return cachedThreads;
  }

  try {
    // Dynamic require avoids hard-failing on older runtimes without node:sqlite.
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const sqlite = require("node:sqlite") as {
      DatabaseSync?: new (p: string, opts?: { readOnly?: boolean }) => {
        prepare: (sql: string) => { all: (...args: unknown[]) => Record<string, unknown>[] };
        close: () => void;
      };
    };
    if (!sqlite?.DatabaseSync) return [];
    const db = new sqlite.DatabaseSync(dbPath, { readOnly: true });
    try {
      const rows = db
        .prepare(
          `SELECT id, path, cwd, name, modified, message_count, COALESCE(first_message, '') AS first_message
           FROM sessions
           ORDER BY modified DESC
           LIMIT ?`,
        )
        .all(MAX_THREADS);
      cachedThreads = rows.map((row) => {
        const id = String(row.id ?? "");
        const name = String(row.name ?? "").trim();
        const firstMsg = String(row.first_message ?? "").replace(/\s+/g, " ").trim();
        const title = name || firstMsg.slice(0, 48) || id.slice(0, 12);
        const msgCount = Number(row.message_count ?? 0);
        const shortId = id.slice(0, 8);
        const subtitle = `${shortId} · ${msgCount} msgs${firstMsg && name ? ` · ${firstMsg.slice(0, 48)}` : ""}`;
        return {
          id,
          title,
          subtitle,
          uri: `grok-pi://session/${id}`,
          insertText: `@session:${shortId}`,
          searchText: `${id} ${title} ${String(row.cwd ?? "")}`.trim(),
          sourcePath: row.path ? String(row.path) : undefined,
        };
      });
      cachedThreadsAt = now;
      return cachedThreads;
    } finally {
      db.close();
    }
  } catch {
    // Rust adapter's `psm_session_catalog::load_catalog` hydrates empty thread lists.
    return [];
  }
}

/**
 * Collect rich MCP server records directly from Pi's own runtime registries
 * (`pi.getMcpServers()` + `pi.getAllTools()`). Zero `mcp.json` disk reads.
 */
function collectMcpServerRecords(pi: ExtensionAPI): McpServerRecord[] {
  const servers = new Map<
    string,
    {
      endpoint: string;
      transport: "http" | "stdio" | "tools";
      tools: Map<string, McpToolRecord>;
    }
  >();

  const ensureServer = (
    name: string,
    endpoint?: string,
    transport?: "http" | "stdio" | "tools",
  ) => {
    const key = name.trim();
    if (!key) return undefined;
    let entry = servers.get(key);
    if (!entry) {
      entry = {
        endpoint: endpoint || "MCP server",
        transport: transport ?? "tools",
        tools: new Map<string, McpToolRecord>(),
      };
      servers.set(key, entry);
    } else {
      if (endpoint && entry.endpoint === "MCP server") entry.endpoint = endpoint;
      if (transport && entry.transport === "tools") entry.transport = transport;
    }
    return entry;
  };

  // 1. Native Pi MCP server registry (`pi.getMcpServers()`).
  try {
    const registered =
      (pi as { getMcpServers?: () => PiMcpServerMeta[] }).getMcpServers?.() ?? [];
    for (const srv of registered) {
      if (!srv?.name) continue;
      if (srv.config?.url) {
        ensureServer(srv.name, srv.config.url, "http");
      } else if (srv.config?.command) {
        ensureServer(srv.name, srv.config.command, "stdio");
      } else {
        ensureServer(srv.name, "MCP server", "tools");
      }
    }
  } catch {
    // Older Pi runtime without getMcpServers; fall through to tool registry.
  }

  // 2. Live tools registered in Pi (`mcp__<server>__<tool>` or `<server>:<tool>`).
  try {
    const tools = ((pi as { getAllTools?: () => PiToolMeta[] }).getAllTools?.() ??
      []) as PiToolMeta[];
    for (const tool of tools) {
      const fullName = String(tool.name ?? "");
      const desc = String(tool.description ?? "").replace(/\s+/g, " ").trim();
      if (fullName.startsWith("mcp__")) {
        const parts = fullName.slice(5).split("__");
        if (parts[0]) {
          const toolName = parts.slice(1).join("__") || fullName;
          const entry = ensureServer(parts[0]);
          entry?.tools.set(toolName, { name: toolName, fullName, description: desc });
        }
      } else if (fullName.includes(":") && !fullName.startsWith("skill:")) {
        const [srv, fn] = fullName.split(":", 2);
        if (srv && fn) {
          const entry = ensureServer(srv);
          entry?.tools.set(fn, { name: fn, fullName, description: desc });
        }
      }
    }
  } catch {
    // Tool registry not ready yet.
  }

  return [...servers.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name, info]) => {
      const tools = [...info.tools.values()].sort((a, b) => a.name.localeCompare(b.name));
      const searchParts = [
        name,
        info.endpoint,
        info.transport,
        ...tools.map((t) => `${t.name} ${t.description}`),
      ];
      return {
        id: name,
        name,
        endpoint: info.endpoint,
        transport: info.transport,
        tools,
        searchText: searchParts.join(" ").trim(),
      };
    });
}

function collectMcpServersFromPi(pi: ExtensionAPI): MentionItem[] {
  return collectMcpServerRecords(pi).map((srv) => {
    const toolNames = srv.tools.map((t) => t.name);
    const subtitle =
      toolNames.length > 0
        ? `${toolNames.length} tools (${toolNames.slice(0, 3).join(", ")})`
        : srv.endpoint || "MCP server";
    return {
      id: srv.id,
      title: srv.name,
      subtitle,
      uri: `grok-pi://mcp/${srv.id}`,
      insertText: `@mcp:${srv.id}`,
      searchText: srv.searchText,
    };
  });
}

function readThreadTailSnippet(jsonlPath: string): string | null {
  if (!jsonlPath || !fs.existsSync(jsonlPath)) return null;
  try {
    const stat = fs.statSync(jsonlPath);
    const readSize = Math.min(stat.size, 16 * 1024);
    const fd = fs.openSync(jsonlPath, "r");
    try {
      const buf = Buffer.alloc(readSize);
      fs.readSync(fd, buf, 0, readSize, Math.max(0, stat.size - readSize));
      const lines = buf.toString("utf8").split("\n").filter(Boolean).slice(-8);
      const excerpts: string[] = [];
      for (const raw of lines) {
        try {
          const obj = JSON.parse(raw) as {
            role?: string;
            message?: { role?: string; content?: unknown };
            content?: unknown;
          };
          const role = obj.message?.role ?? obj.role;
          const content = obj.message?.content ?? obj.content;
          if (!role || !content) continue;
          const text =
            typeof content === "string"
              ? content
              : Array.isArray(content)
                ? content
                    .map((p: { text?: string }) => (typeof p?.text === "string" ? p.text : ""))
                    .join(" ")
                : "";
          const clean = text.replace(/\s+/g, " ").trim();
          if (clean) excerpts.push(`[${role}] ${clean.slice(0, 200)}`);
        } catch {
          // Skip partial boundary line.
        }
      }
      return excerpts.length > 0 ? excerpts.slice(-4).join("\n") : null;
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    return null;
  }
}

/**
 * Describe an `@folder` reference so the model knows what the folder holds.
 *
 * The Pager inserts `@path` as literal text and Pi core does not expand it, so
 * without this a folder reference reaches the model as a bare path and it has
 * to guess (or spend a tool call) to learn the contents. Returns `null` when the
 * path is not a directory, which keeps ordinary `@file` references untouched.
 *
 * One level only, sorted, capped at {@link MAX_DIR_ENTRIES}: the point is an
 * accurate outline, not a full recursive dump.
 */
function describeFolderReference(rawPath: string, cwd: string): string | null {
  try {
    const abs = path.isAbsolute(rawPath) ? rawPath : path.resolve(cwd, rawPath);
    const stat = fs.statSync(abs);
    if (!stat.isDirectory()) return null;

    const entries = fs.readdirSync(abs, { withFileTypes: true });
    const dirs: string[] = [];
    const files: string[] = [];
    for (const entry of entries) {
      // Dotfiles are noise in an outline; the model can still reach them explicitly.
      if (entry.name.startsWith(".")) continue;
      (entry.isDirectory() ? dirs : files).push(entry.name);
    }
    dirs.sort((a, b) => a.localeCompare(b));
    files.sort((a, b) => a.localeCompare(b));

    const shown = [...dirs.map((d) => `${d}/`), ...files];
    const truncated = shown.length > MAX_DIR_ENTRIES;
    const listed = shown.slice(0, MAX_DIR_ENTRIES);
    const heading = `- **grok-pi://folder/${rawPath}** — workspace folder \`${rawPath}\` containing ${shown.length}${truncated ? "+" : ""} entries (one level):`;
    const lines = listed.map((name) => `  - ${name}`);
    if (truncated) {
      lines.push(`  - … ${shown.length - MAX_DIR_ENTRIES} more; list the folder for the rest`);
    }
    return [heading, ...lines].join("\n");
  } catch {
    return null;
  }
}

function buildBuiltInProviders(pi: ExtensionAPI): MentionProviderSpec[] {
  return [
    {
      kind: "skill",
      aliases: ["技能", "sk"],
      label: "Skill",
      description: "Reference a Pi-registered skill (grok-pi://skill/<name>)",
      action: {
        key: "ctrl+o",
        label: "skill picker",
        command: SKILL_PICKER_COMMAND,
      },
      collect: () => collectSkillsFromPi(pi),
      resolve: (id) => {
        const items = collectSkillsFromPi(pi);
        const found = items.find((i) => i.id === id || i.id.toLowerCase() === id.toLowerCase());
        if (!found) return null;
        return `- **${found.uri}** (${found.title})${found.sourcePath ? ` — file: \`${found.sourcePath}\`` : ""}${found.subtitle ? ` — ${found.subtitle}` : ""}`;
      },
    },
    {
      kind: "session",
      aliases: ["会话", "对话", "历史", "thread"],
      label: "Session",
      description: "Reference a past Pi session via PSM SQLite index (grok-pi://session/<id>)",
      action: {
        key: "ctrl+o",
        label: "session picker",
        command: SESSION_PICKER_COMMAND,
      },
      collect: () => collectThreadsFromPsmDb(),
      resolve: (id) => {
        const records = loadSessionPickerRecords({ limit: 500 });
        const foundRec = records.find((r) => r.id === id || r.id.startsWith(id));
        if (foundRec) {
          const preview = loadSessionPreviewMessages(foundRec.path, { maxMessages: 6 });
          const excerpt = preview
            .slice(-4)
            .map((m) => `[${m.role}] ${m.content.replace(/\s+/g, " ").trim().slice(0, 200)}`)
            .join("\n");
          const meta = `${foundRec.project}, ${foundRec.messageCount} msgs, ${formatTokensM(foundRec.totalTokens)}, ${foundRec.model}`;
          return `- **grok-pi://session/${foundRec.id}** (${foundRec.title} — ${meta}, path: \`${foundRec.path}\`)${excerpt ? `\n  Recent excerpt:\n  ${excerpt.replace(/\n/g, "\n  ")}` : ""}`;
        }
        const items = collectThreadsFromPsmDb();
        const found = items.find((i) => i.id === id || i.id.startsWith(id));
        if (!found) return `- **grok-pi://session/${id}** (referenced past session ID)`;
        const snippet = found.sourcePath ? readThreadTailSnippet(found.sourcePath) : null;
        return `- **${found.uri}** (${found.title}${found.sourcePath ? `, path: \`${found.sourcePath}\`` : ""})${snippet ? `\n  Recent excerpt:\n  ${snippet.replace(/\n/g, "\n  ")}` : ""}`;
      },
    },
    {
      kind: "file",
      aliases: ["文件", "f", "path"],
      label: "File",
      description: "Browse tree, fd smart search, or rg full-text grep (@file:<path>)",
      action: {
        key: "ctrl+o",
        label: "file explorer",
        command: FILE_PICKER_COMMAND,
      },
      // Rust's FuzzyFileMatcherDaemon owns fast inline file dropdown rows.
      collect: () => [],
    },
    {
      kind: "mcp",
      aliases: ["工具服务"],
      label: "MCP",
      description: "Reference a Pi-registered MCP server (grok-pi://mcp/<name>)",
      action: {
        key: "ctrl+o",
        label: "mcp picker",
        command: MCP_PICKER_COMMAND,
      },
      collect: () => collectMcpServersFromPi(pi),
      resolve: (id) => {
        const items = collectMcpServersFromPi(pi);
        const found = items.find((i) => i.id === id || i.id.toLowerCase() === id.toLowerCase());
        if (!found) return `- **grok-pi://mcp/${id}**`;
        return `- **${found.uri}** (${found.subtitle})`;
      },
    },
  ];
}

export default function mentions(pi: ExtensionAPI) {
  const builtIns = buildBuiltInProviders(pi);

  const allProviders = (): MentionProviderSpec[] => {
    const merged = new Map<string, MentionProviderSpec>();
    for (const p of builtIns) merged.set(p.kind, p);
    for (const [k, p] of customProviders) merged.set(k, p);
    return [...merged.values()];
  };

  const publishCatalog = (ui: PatchableUi | undefined, cwd: string, force = false): void => {
    if (!isGrokPiHost() || !ui || typeof ui.setStatus !== "function") return;
    if (force) {
      cachedThreadsAt = 0;
    }
    const providers: SerializedProvider[] = allProviders().map((p) => ({
      kind: p.kind,
      aliases: p.aliases,
      label: p.label,
      description: p.description,
      ...(p.action ? { action: p.action } : {}),
      items: p.collect({ cwd }).map(({ sourcePath: _omit, ...rest }) => rest),
    }));
    const payload = JSON.stringify({ providers });
    if (!force && payload === lastPublishedPayload) return;
    if (payload.length <= MAX_CATALOG_BYTES) {
      try {
        ui.setStatus(STATUS_KEY, payload);
        lastPublishedPayload = payload;
      } catch {
        // RPC transport closed; ignore.
      }
    }
  };

  // Hidden command invoked by Rust Pager when the user presses `@session`'s
  // dynamic shortcut (`Ctrl+O`) while `@session` is active in the dropdown.
  pi.registerCommand(SESSION_PICKER_COMMAND, {
    description: "Open the bordered TS session picker overlay for @session mentions",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const cwd = (ctx as { cwd?: string }).cwd || lastCwd || process.cwd();
      const records = loadSessionPickerRecords({ limit: 300 });
      const initialQuery = String(args ?? "").trim();

      const chosen = await ctx.ui.custom<SessionRecord | undefined>(
        (tui, theme, _kb, done) =>
          createSessionPickerTui(
            tui as { requestRender: () => void },
            theme as SessionPickerTheme,
            {
              records,
              currentCwd: cwd,
              initialQuery,
              initialScope: "global",
              getRows: () => {
                const tRows = (tui as { terminal?: { rows?: number } }).terminal?.rows;
                return typeof tRows === "number" && tRows > 0 ? tRows : 28;
              },
            },
            done,
          ),
        {
          overlay: true,
          overlayOptions: {
            width: "92%",
            maxHeight: "88%",
            anchor: "center",
          },
        },
      );

      if (!chosen) return;

      const insertText = `@session:${chosen.shortId}`;
      emitMentionInsert(ctx, "session", chosen.id, chosen.title, insertText);
    },
  });

  // Hidden command invoked by Rust Pager when the user presses `@skill`'s
  // dynamic shortcut (`Ctrl+O`) while `@skill` is active in the dropdown.
  pi.registerCommand(SKILL_PICKER_COMMAND, {
    description: "Open the bordered TS skill picker overlay for @skill mentions",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const cwd = (ctx as { cwd?: string }).cwd || lastCwd || process.cwd();
      const records = buildSkillRecords(collectSkillsFromPi(pi), cwd);
      const initialQuery = String(args ?? "").trim();

      const chosen = await ctx.ui.custom<SkillRecord | undefined>(
        (tui, theme, _kb, done) =>
          createSkillPickerTui(
            tui as { requestRender: () => void },
            theme as SessionPickerTheme,
            {
              records,
              initialQuery,
              initialScope: "global",
              getRows: () => {
                const tRows = (tui as { terminal?: { rows?: number } }).terminal?.rows;
                return typeof tRows === "number" && tRows > 0 ? tRows : 28;
              },
            },
            done,
          ),
        {
          overlay: true,
          overlayOptions: {
            width: "92%",
            maxHeight: "88%",
            anchor: "center",
          },
        },
      );

      if (!chosen) return;
      emitMentionInsert(ctx, "skill", chosen.id, chosen.title, `@skill:${chosen.id}`);
    },
  });

  // Hidden command invoked by Rust Pager when the user presses `@file`'s
  // dynamic shortcut (`Ctrl+O`) while `@file` or a workspace file row is active.
  pi.registerCommand(FILE_PICKER_COMMAND, {
    description: "Open the bordered TS file explorer (Tree + fd + rg + Preview)",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const cwd = (ctx as { cwd?: string }).cwd || lastCwd || process.cwd();
      const initialQuery = String(args ?? "").trim();

      const chosen = await ctx.ui.custom<FilePickerSelection | undefined>(
        (tui, theme, _kb, done) =>
          createFilePickerTui(
            tui as { requestRender: () => void },
            theme as SessionPickerTheme,
            {
              rootDir: cwd,
              initialQuery,
              getRows: () => {
                const tRows = (tui as { terminal?: { rows?: number } }).terminal?.rows;
                return typeof tRows === "number" && tRows > 0 ? tRows : 28;
              },
            },
            done,
          ),
        {
          overlay: true,
          overlayOptions: {
            width: "92%",
            maxHeight: "88%",
            anchor: "center",
          },
        },
      );

      if (!chosen) return;
      emitMentionInsert(ctx, "file", chosen.title, chosen.title, chosen.insertText);
    },
  });

  // Hidden command invoked by Rust Pager when the user presses `@mcp`'s
  // dynamic shortcut (`Ctrl+O`) while `@mcp` is active in the dropdown.
  pi.registerCommand(MCP_PICKER_COMMAND, {
    description: "Open the bordered TS MCP server & tool picker overlay for @mcp mentions",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const records = collectMcpServerRecords(pi);
      const initialQuery = String(args ?? "").trim();

      const chosen = await ctx.ui.custom<McpServerRecord | undefined>(
        (tui, theme, _kb, done) =>
          createMcpPickerTui(
            tui as { requestRender: () => void },
            theme as SessionPickerTheme,
            {
              records,
              initialQuery,
              getRows: () => {
                const tRows = (tui as { terminal?: { rows?: number } }).terminal?.rows;
                return typeof tRows === "number" && tRows > 0 ? tRows : 28;
              },
            },
            done,
          ),
        {
          overlay: true,
          overlayOptions: {
            width: "92%",
            maxHeight: "88%",
            anchor: "center",
          },
        },
      );

      if (!chosen) return;
      emitMentionInsert(ctx, "mcp", chosen.id, chosen.name, `@mcp:${chosen.id}`);
    },
  });

  // Hidden command invoked by Rust Pager when `/resume` runs inside an active
  // `grok-pi` session, reusing the shared bordered TS `SessionPickerTui`.
  pi.registerCommand(RESUME_SESSION_COMMAND, {
    description: "Open the bordered TS session picker overlay to resume a past session",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const cwd = (ctx as { cwd?: string }).cwd || lastCwd || process.cwd();
      const records = loadSessionPickerRecords({ limit: 300 });
      const initialQuery = String(args ?? "").trim();

      const chosen = await ctx.ui.custom<SessionRecord | undefined>(
        (tui, theme, _kb, done) =>
          createSessionPickerTui(
            tui as { requestRender: () => void },
            theme as SessionPickerTheme,
            {
              records,
              currentCwd: cwd,
              initialQuery,
              initialScope: "global",
              titleBadge: "(/resume)",
              confirmLabel: "resume",
              getRows: () => {
                const tRows = (tui as { terminal?: { rows?: number } }).terminal?.rows;
                return typeof tRows === "number" && tRows > 0 ? tRows : 28;
              },
            },
            done,
          ),
        {
          overlay: true,
          overlayOptions: {
            width: "92%",
            maxHeight: "88%",
            anchor: "center",
          },
        },
      );

      if (!chosen) return;
      if (isGrokPiHost()) {
        emitMentionInsert(ctx, "resume", chosen.id, chosen.title, chosen.cwd);
      } else {
        await ctx.switchSession(chosen.path);
      }
    },
  });

  function emitMentionInsert(
    ctx: ExtensionCommandContext,
    kind: string,
    id: string,
    title: string,
    insertText: string,
  ): void {
    if (isGrokPiHost()) {
      const ui = ctx.ui as unknown as PatchableUi;
      try {
        ui.setStatus?.(
          INSERT_STATUS_KEY,
          JSON.stringify({
            kind,
            id,
            title,
            insertText,
            nonce: Date.now(),
          }),
        );
        ui.setStatus?.(INSERT_STATUS_KEY, undefined);
      } catch {
        // RPC transport closed; ignore.
      }
    } else {
      const editorUi = ctx.ui as unknown as {
        getEditorText?: () => string;
        setEditorText?: (text: string) => void;
      };
      if (typeof editorUi.setEditorText === "function") {
        const existing =
          typeof editorUi.getEditorText === "function" ? editorUi.getEditorText() : "";
        const sep = existing && !/\s$/.test(existing) ? " " : "";
        editorUi.setEditorText(`${existing}${sep}${insertText} `);
      }
    }
  }

  const g = globalThis as GlobalWithMentions;
  g.__piGrokRegisterMentionProvider = (provider: MentionProviderSpec) => {
    customProviders.set(provider.kind, provider);
    publishCatalog(lastUi, lastCwd, true);
  };
  g.__piGrokRefreshMentions = () => {
    publishCatalog(lastUi, lastCwd, true);
  };

  pi.on("session_start", (_event, ctx) => {
    lastUi = ctx.ui as PatchableUi;
    lastCwd = (ctx as { cwd?: string }).cwd || process.cwd();
    publishCatalog(lastUi, lastCwd, true);
  });

  pi.on("session_switch", (_event, ctx) => {
    lastUi = ctx.ui as PatchableUi;
    lastCwd = (ctx as { cwd?: string }).cwd || lastCwd;
    publishCatalog(lastUi, lastCwd, true);
  });

  // Hydrate Pi's native skill list + resolve `@kind:id` mentions for BOTH
  // `grok-pi` and standalone `pi` agent runs.
  pi.on("before_agent_start", (event, ctx) => {
    const cwd = (ctx as { cwd?: string }).cwd || lastCwd;

    // 1. Sync Pi's live skill metadata (zero disk scan) and publish if catalog changed.
    const promptSkills = (
      (event as { systemPromptOptions?: { skills?: PiSkillMeta[] } }).systemPromptOptions?.skills ??
      []
    ) as PiSkillMeta[];
    let skillCacheUpdated = false;
    for (const s of promptSkills) {
      if (!s?.name || s.disableModelInvocation) continue;
      const before = cachedPiSkills.get(s.name);
      if (!before || before.sourcePath !== s.filePath) {
        upsertSkill(s.name, s.description, s.filePath);
        skillCacheUpdated = true;
      }
    }
    if (skillCacheUpdated) {
      publishCatalog(lastUi, cwd);
    }

    // 2. Resolve explicit `@kind:id` / `@kind：id` references in the user prompt.
    const sections = (event.systemPromptOptions.sections ??= {});
    const prompt = String((event as { prompt?: string }).prompt ?? "");
    if (!prompt.includes("@")) {
      delete sections.referenced_objects;
      return;
    }

    // Match `@<kind>:<id>` or `@<kind>：<id>` (supports ASCII & Chinese aliases).
    const re = /@([\p{L}\p{N}_-]+)[:：]([^\s,;]+)/gu;
    const providers = allProviders();
    const resolvedLines: string[] = [];
    const seen = new Set<string>();

    for (const match of prompt.matchAll(re)) {
      const rawKind = (match[1] ?? "").toLowerCase();
      const rawId = match[2] ?? "";
      if (!rawKind || !rawId) continue;

      if (["file", "f", "path", "文件"].includes(rawKind)) {
        const key = `file:${rawId}`;
        if (!seen.has(key)) {
          seen.add(key);
          // `@file:` can name a folder too (dir mode reuses the same token), so
          // resolve the real type instead of always claiming it is a file.
          resolvedLines.push(
            describeFolderReference(rawId, cwd) ??
              `- **grok-pi://file/${rawId}** — workspace file \`${rawId}\``,
          );
        }
        continue;
      }

      const provider = providers.find(
        (p) =>
          p.kind.toLowerCase() === rawKind ||
          p.aliases.some((a) => a.toLowerCase() === rawKind),
      );
      if (!provider) continue;

      const key = `${provider.kind}:${rawId}`;
      if (seen.has(key)) continue;
      seen.add(key);

      const line =
        provider.resolve?.(rawId, { cwd }) ?? `- **grok-pi://${provider.kind}/${rawId}**`;
      if (line) resolvedLines.push(line);
    }

    // 3. Resolve bare `@path` references that point at a folder.
    //
    // The Pager drops a plain `@crates/common` into the prompt verbatim, and Pi
    // core does not expand it, so the model would receive a path with no idea
    // what is inside. Only directories are expanded here: a bare file path is
    // already handled well enough by the model's own read tool, and expanding
    // every stray `@word` in prose would be noise.
    // Capture the whole token after `@` (including any `:`), then reject the
    // forms another pass already owns. Matching only up to the colon would
    // grab the bare kind word out of `@skill:...` and `@file:...`.
    const bare = /(^|[\s,;])@([^\s,;]+)/gu;
    for (const match of prompt.matchAll(bare)) {
      const rawPath = match[2] ?? "";
      if (!rawPath || rawPath.startsWith("!")) continue;
      // `@kind:id` is the kind pass's job.
      if (rawPath.includes(":") || rawPath.includes("：")) continue;
      // A path with a line range (`@src/main.rs:10`) never reaches here because
      // the colon check above rejects it; this also skips trailing punctuation
      // such as `@crates/common.` at a sentence end.
      const candidate = rawPath.replace(/[.,;:!?)\]}]+$/u, "");
      if (!candidate) continue;
      const key = `folder:${candidate}`;
      if (seen.has(key)) continue;
      const described = describeFolderReference(candidate, cwd);
      if (!described) continue;
      seen.add(key);
      resolvedLines.push(described);
    }

    if (resolvedLines.length > 0) {
      sections.referenced_objects = `The user explicitly referenced these structured objects via \`@\` mentions:\n${resolvedLines.join("\n")}`;
    } else {
      delete sections.referenced_objects;
    }
  });
}
