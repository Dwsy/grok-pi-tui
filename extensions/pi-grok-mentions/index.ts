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
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** Must match `EXTENSION_MENTIONS_STATUS_KEY` in pi-grok-adapter. */
const STATUS_KEY = "__pi_grok_mentions__";
const MAX_THREADS = 50;
const MAX_CATALOG_BYTES = 450 * 1024;
const THREAD_CACHE_TTL_MS = 5_000;

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

export interface MentionProviderSpec {
  kind: string;
  aliases: string[];
  label: string;
  description: string;
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
 * Collect MCP servers directly from Pi's own runtime registries
 * (`pi.getMcpServers()` + `pi.getAllTools()`). Zero `mcp.json` disk reads.
 */
function collectMcpServersFromPi(pi: ExtensionAPI): MentionItem[] {
  const servers = new Map<string, { tools: string[]; description: string }>();

  const addServer = (name: string, tool?: string, desc?: string) => {
    const key = name.trim();
    if (!key) return;
    const entry = servers.get(key) ?? { tools: [], description: desc ?? "" };
    if (tool && !entry.tools.includes(tool)) entry.tools.push(tool);
    if (!entry.description && desc) entry.description = desc;
    servers.set(key, entry);
  };

  // 1. Native Pi MCP server registry (`pi.getMcpServers()`).
  try {
    const registered =
      (pi as { getMcpServers?: () => PiMcpServerMeta[] }).getMcpServers?.() ?? [];
    for (const srv of registered) {
      if (!srv?.name) continue;
      const detail = srv.config?.url ?? srv.config?.command ?? "MCP server";
      addServer(srv.name, undefined, detail);
    }
  } catch {
    // Older Pi runtime without getMcpServers; fall through to tool registry.
  }

  // 2. Live tools registered in Pi (`mcp__<server>__<tool>` or `<server>:<tool>`).
  try {
    const tools = ((pi as { getAllTools?: () => PiToolMeta[] }).getAllTools?.() ??
      []) as PiToolMeta[];
    for (const tool of tools) {
      const name = String(tool.name ?? "");
      if (name.startsWith("mcp__")) {
        const parts = name.slice(5).split("__");
        if (parts[0]) addServer(parts[0], parts.slice(1).join("__") || name);
      } else if (name.includes(":") && !name.startsWith("skill:")) {
        const [srv, fn] = name.split(":", 2);
        if (srv) addServer(srv, fn);
      }
    }
  } catch {
    // Tool registry not ready yet.
  }

  return [...servers.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name, info]) => {
      const subtitle =
        info.tools.length > 0
          ? `${info.tools.length} tools (${info.tools.slice(0, 3).join(", ")})`
          : info.description || "MCP server";
      return {
        id: name,
        title: name,
        subtitle,
        uri: `grok-pi://mcp/${name}`,
        insertText: `@mcp:${name}`,
        searchText: `${name} ${info.tools.join(" ")} ${info.description}`.trim(),
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

function buildBuiltInProviders(pi: ExtensionAPI): MentionProviderSpec[] {
  return [
    {
      kind: "skill",
      aliases: ["技能", "sk"],
      label: "Skill",
      description: "Reference a Pi-registered skill (grok-pi://skill/<name>)",
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
      collect: () => collectThreadsFromPsmDb(),
      resolve: (id) => {
        const items = collectThreadsFromPsmDb();
        const found = items.find((i) => i.id === id || i.id.startsWith(id));
        if (!found) return `- **grok-pi://session/${id}** (referenced past session ID)`;
        const snippet = found.sourcePath ? readThreadTailSnippet(found.sourcePath) : null;
        return `- **${found.uri}** (${found.title}${found.sourcePath ? `, path: \`${found.sourcePath}\`` : ""})${snippet ? `\n  Recent excerpt:\n  ${snippet.replace(/\n/g, "\n  ")}` : ""}`;
      },
    },
    {
      kind: "mcp",
      aliases: ["工具服务"],
      label: "MCP",
      description: "Reference a Pi-registered MCP server (grok-pi://mcp/<name>)",
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
          resolvedLines.push(`- **grok-pi://file/${rawId}** — workspace file \`${rawId}\``);
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

    if (resolvedLines.length > 0) {
      sections.referenced_objects = `The user explicitly referenced these structured objects via \`@\` mentions:\n${resolvedLines.join("\n")}`;
    } else {
      delete sections.referenced_objects;
    }
  });
}
