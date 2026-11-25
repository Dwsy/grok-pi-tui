/**
 * Full-session JSONL trace parser & filter for `/trace` and Left-arrow tool
 * trace inspection (`__pi_trace`).
 *
 * Unlike the message-only session preview, `/trace` parses EVERY line of the
 * session JSONL file — pairing `toolCall`/`tool_use` blocks with their
 * corresponding `toolResult`/`tool_result` outputs while preserving user
 * prompts, assistant replies, thinking blocks, and metadata entries
 * (`session`, `model_change`, `compaction`, `session_info`, `label`, etc.).
 */

import fs from "node:fs";
import { formatTimestamp, formatTokensM } from "../pi-grok-mentions/session-data.ts";

export type TraceEntryKind = "tool" | "user" | "assistant" | "thinking" | "meta";
export type TraceFilter = "all" | TraceEntryKind;

export interface TraceTokenUsage {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

export interface TraceEntry {
  /** 1-based display index in the parsed trace list. */
  index: number;
  /** 1-based line number in the source JSONL file. */
  lineNumber: number;
  id: string;
  /** Tool call identifier (e.g. `call_...`) when `kind === "tool"`. */
  toolCallId?: string;
  kind: TraceEntryKind;
  rawType: string;
  label: string;
  subtitle: string;
  timestamp: string;
  model?: string;
  usage?: TraceTokenUsage;
  toolName?: string;
  toolInput?: string;
  toolOutput?: string;
  toolArgs?: unknown;
  toolResultPayload?: { content: unknown[]; details?: unknown };
  customType?: string;
  customMessage?: Record<string, unknown>;
  isError?: boolean;
  previewBody: string;
  rawJson: string;
  /** Precomputed lowercase search haystack for fast keystroke filtering. */
  searchLower: string;
}

function formatCompactJson(value: unknown, maxChars = 16_000): string {
  if (value === undefined || value === null) return "";
  if (typeof value === "string") {
    return value.length > maxChars ? `${value.slice(0, maxChars)}\n… (truncated)` : value;
  }
  try {
    const pretty = JSON.stringify(value, null, 2);
    return pretty.length > maxChars ? `${pretty.slice(0, maxChars)}\n… (truncated)` : pretty;
  } catch {
    return String(value);
  }
}

function summarizeOneLine(text: string, maxLen = 72): string {
  const clean = text.replace(/\s+/g, " ").trim();
  if (!clean) return "";
  return clean.length > maxLen ? `${clean.slice(0, maxLen - 1)}…` : clean;
}

function extractPlainText(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  const parts: string[] = [];
  for (const block of content) {
    if (!block || typeof block !== "object") continue;
    const item = block as { type?: unknown; text?: unknown };
    const type = typeof item.type === "string" ? item.type.toLowerCase() : "";
    if (
      (type === "" || type === "text" || type === "output_text" || type === "input_text") &&
      typeof item.text === "string"
    ) {
      if (item.text.trim()) parts.push(item.text);
    }
  }
  return parts.join("\n").trim();
}

function extractThinkingText(content: unknown): string {
  if (!Array.isArray(content)) return "";
  const parts: string[] = [];
  for (const block of content) {
    if (!block || typeof block !== "object") continue;
    const item = block as { type?: unknown; thinking?: unknown; text?: unknown };
    const type = typeof item.type === "string" ? item.type.toLowerCase() : "";
    if (type === "thinking" || type === "redacted_thinking") {
      const text = typeof item.thinking === "string" ? item.thinking : typeof item.text === "string" ? item.text : "";
      if (text.trim()) parts.push(text.trim());
    }
  }
  return parts.join("\n\n").trim();
}

function extractUsage(usage: unknown): TraceTokenUsage | undefined {
  if (!usage || typeof usage !== "object") return undefined;
  const u = usage as Record<string, unknown>;
  const input = Number(u.input ?? u.input_tokens ?? 0) || 0;
  const output = Number(u.output ?? u.output_tokens ?? 0) || 0;
  const cacheRead = Number(u.cacheRead ?? u.cache_read ?? u.cache_read_tokens ?? 0) || 0;
  const cacheWrite = Number(u.cacheWrite ?? u.cache_write ?? u.cache_write_tokens ?? 0) || 0;
  if (input === 0 && output === 0 && cacheRead === 0 && cacheWrite === 0) return undefined;
  return { input, output, cacheRead, cacheWrite };
}

function summarizeToolArgs(name: string, args: unknown): string {
  if (!args || typeof args !== "object") return name;
  const obj = args as Record<string, unknown>;
  const primary =
    obj.path ?? obj.file_path ?? obj.command ?? obj.cmd ?? obj.query ?? obj.pattern ?? obj.url;
  if (typeof primary === "string" && primary.trim()) {
    return `${name} · ${summarizeOneLine(primary, 52)}`;
  }
  return name;
}

function formatByteHint(text: string): string {
  const len = Buffer.byteLength(text, "utf8");
  if (len < 1024) return `${len}B`;
  return `${(len / 1024).toFixed(1)}KB`;
}

function buildSearchLower(entry: {
  id: string;
  toolCallId?: string;
  kind: string;
  rawType: string;
  label: string;
  subtitle: string;
  model?: string;
  toolName?: string;
  toolInput?: string;
  toolOutput?: string;
  previewBody: string;
}): string {
  return [
    entry.id,
    entry.toolCallId ?? "",
    entry.kind,
    entry.rawType,
    entry.label,
    entry.subtitle,
    entry.model ?? "",
    entry.toolName ?? "",
    (entry.toolInput ?? "").slice(0, 4000),
    (entry.toolOutput ?? "").slice(0, 4000),
    entry.previewBody.slice(0, 4000),
  ]
    .join("\n")
    .toLowerCase();
}

const traceParseCache = new Map<string, { mtimeMs: number; size: number; entries: TraceEntry[] }>();

/** Attach a lazy `rawJson` getter so `JSON.stringify(..., 2)` runs only when inspected. */
function attachLazyRawJson(entry: TraceEntry, rawSource: unknown): TraceEntry {
  let cached: string | undefined;
  Object.defineProperty(entry, "rawJson", {
    enumerable: true,
    configurable: true,
    get() {
      if (cached === undefined) {
        cached = formatCompactJson(rawSource, 24_000);
      }
      return cached;
    },
  });
  return entry;
}

/**
 * Parse every line of a Pi session JSONL file into structured trace entries.
 * Pairs `toolCall`/`tool_use` entries with their matching `toolResult` output
 * by `toolCallId` so tool traces display Input + Output side-by-side in one view.
 */
export function parseSessionTraceJsonl(jsonlPath: string): TraceEntry[] {
  if (!jsonlPath || !fs.existsSync(jsonlPath)) return [];
  let stat: fs.Stats | undefined;
  try {
    stat = fs.statSync(jsonlPath);
    const cached = traceParseCache.get(jsonlPath);
    if (cached && cached.mtimeMs === stat.mtimeMs && cached.size === stat.size) {
      return cached.entries;
    }
  } catch {
    return [];
  }
  let raw: string;
  try {
    raw = fs.readFileSync(jsonlPath, "utf8");
  } catch {
    return [];
  }

  const entries: TraceEntry[] = [];
  const toolEntryByCallId = new Map<string, TraceEntry>();
  const lines = raw.split("\n");

  const pushEntry = (
    draft: Omit<TraceEntry, "index" | "searchLower" | "rawJson"> & { rawJson?: string },
    rawSource?: unknown,
  ): TraceEntry => {
    const entry: TraceEntry = {
      ...draft,
      rawJson: draft.rawJson ?? "",
      index: entries.length + 1,
      searchLower: buildSearchLower(draft),
    };
    if (draft.rawJson === undefined && rawSource !== undefined) {
      attachLazyRawJson(entry, rawSource);
    }
    entries.push(entry);
    return entry;
  };

  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i]!.trim();
    if (!trimmed || trimmed[0] !== "{") continue;
    const lineNumber = i + 1;
    let obj: Record<string, unknown>;
    try {
      obj = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      continue;
    }

    const rawType = typeof obj.type === "string" ? obj.type : "entry";
    const entryId = typeof obj.id === "string" && obj.id ? obj.id : `line-${lineNumber}`;
    const timestamp = typeof obj.timestamp === "string" ? obj.timestamp : "";

    if (rawType !== "message") {
      const rawJson = formatCompactJson(obj, 24_000);
      let label = rawType;
      let subtitle = `L${lineNumber}`;
      if (rawType === "session") {
        const cwd = typeof obj.cwd === "string" ? obj.cwd : "";
        label = `session · ${summarizeOneLine(cwd || entryId, 56)}`;
        subtitle = `id ${entryId.slice(0, 8)} · L${lineNumber}`;
      } else if (rawType === "session_info") {
        const name = typeof obj.name === "string" ? obj.name : "";
        label = `session_info · ${summarizeOneLine(name || "(unnamed)", 56)}`;
      } else if (rawType === "model_change") {
        const model = String(obj.modelId ?? obj.model ?? "");
        const provider = String(obj.provider ?? "");
        label = `model_change · ${provider ? `${provider}/${model}` : model}`;
      } else if (rawType === "compaction") {
        const summary = typeof obj.summary === "string" ? obj.summary : "";
        label = `compaction · ${summarizeOneLine(summary || "context compacted", 56)}`;
      } else if (rawType === "label") {
        label = `label · ${summarizeOneLine(String(obj.label ?? ""), 56)}`;
      } else if (rawType === "custom_message") {
        const customType = typeof obj.customType === "string" ? obj.customType : "custom";
        label = `custom · ${customType}`;
      }
      const customType =
        rawType === "custom_message" && typeof obj.customType === "string"
          ? obj.customType
          : undefined;
      pushEntry({
        lineNumber,
        id: entryId,
        kind: "meta",
        rawType,
        label,
        subtitle,
        timestamp,
        customType,
        customMessage: customType ? obj : undefined,
        previewBody: rawJson,
        rawJson,
      });
      continue;
    }

    const msg = (obj.message ?? obj) as {
      role?: unknown;
      content?: unknown;
      model?: unknown;
      provider?: unknown;
      usage?: unknown;
      toolCallId?: unknown;
      tool_call_id?: unknown;
      toolName?: unknown;
      isError?: unknown;
      timestamp?: unknown;
    };
    const role = typeof msg.role === "string" ? msg.role.toLowerCase() : "";
    const msgTs =
      timestamp ||
      (typeof msg.timestamp === "string"
        ? msg.timestamp
        : typeof msg.timestamp === "number"
          ? new Date(msg.timestamp).toISOString()
          : "");
    const model =
      typeof msg.model === "string" && msg.model
        ? typeof msg.provider === "string" && msg.provider && !msg.model.includes("/")
          ? `${msg.provider}/${msg.model}`
          : msg.model
        : undefined;
    const usage = extractUsage(msg.usage);

    if (role === "user") {
      const text = extractPlainText(msg.content);
      pushEntry({
        lineNumber,
        id: entryId,
        kind: "user",
        rawType: "user",
        label: `User · ${summarizeOneLine(text || "(empty)", 60)}`,
        subtitle: `L${lineNumber} · ${formatByteHint(text)}`,
        timestamp: msgTs,
        previewBody: text || "(empty user message)",
      }, obj);
      continue;
    }

    if (role === "toolresult" || role === "tool_result") {
      const callId = String(msg.toolCallId ?? msg.tool_call_id ?? "").trim();
      const toolName = typeof msg.toolName === "string" ? msg.toolName : "tool";
      const outputText = extractPlainText(msg.content) || formatCompactJson(msg.content);
      const isError = msg.isError === true;
      const resultContent = Array.isArray(msg.content)
        ? msg.content
        : typeof msg.content === "string"
          ? [{ type: "text", text: msg.content }]
          : [];
      const resultPayload = { content: resultContent, details: msg.details };
      const matched = callId ? toolEntryByCallId.get(callId) : undefined;
      if (matched) {
        matched.toolOutput = outputText;
        matched.toolResultPayload = resultPayload;
        matched.isError = isError;
        matched.subtitle = `${matched.toolCallId?.slice(0, 12) ?? matched.id.slice(0, 8)} · ${isError ? "error" : "ok"} · ${formatByteHint(outputText)}`;
        matched.previewBody = [
          "── Input ──",
          matched.toolInput || "(none)",
          "",
          `── Output (${isError ? "error" : "ok"}) ──`,
          outputText || "(empty)",
        ].join("\n");
        matched.searchLower = buildSearchLower(matched);
      } else {
        pushEntry({
          lineNumber,
          id: entryId,
          toolCallId: callId || undefined,
          kind: "tool",
          rawType: "toolResult",
          toolName,
          label: `${toolName} · result`,
          subtitle: `${callId ? `${callId.slice(0, 12)} · ` : ""}${isError ? "error" : "ok"} · L${lineNumber}`,
          timestamp: msgTs,
          toolOutput: outputText,
          toolResultPayload: resultPayload,
          isError,
          previewBody: `── Output (${isError ? "error" : "ok"}) ──\n${outputText || "(empty)"}`,
        }, obj);
      }
      continue;
    }

    if (role === "assistant") {
      const thinking = extractThinkingText(msg.content);
      if (thinking) {
        pushEntry({
          lineNumber,
          id: `${entryId}:thinking`,
          kind: "thinking",
          rawType: "thinking",
          label: `Thinking · ${summarizeOneLine(thinking, 56)}`,
          subtitle: `${model ? `${model.split("/").pop()} · ` : ""}${formatByteHint(thinking)} · L${lineNumber}`,
          timestamp: msgTs,
          model,
          usage,
          previewBody: thinking,
        }, obj);
      }

      // Extract tool calls from assistant content blocks.
      if (Array.isArray(msg.content)) {
        let toolIdx = 0;
        for (const block of msg.content) {
          if (!block || typeof block !== "object") continue;
          const item = block as {
            type?: unknown;
            id?: unknown;
            name?: unknown;
            arguments?: unknown;
            input?: unknown;
          };
          const bType = typeof item.type === "string" ? item.type.toLowerCase() : "";
          if (bType !== "toolcall" && bType !== "tool_call" && bType !== "tool_use") continue;
          toolIdx += 1;
          const callId = typeof item.id === "string" && item.id ? item.id : `${entryId}:tool-${toolIdx}`;
          const toolName = typeof item.name === "string" && item.name ? item.name : "tool";
          const args = item.arguments ?? item.input;
          const inputText = formatCompactJson(args);
          const created = pushEntry({
            lineNumber,
            id: callId,
            toolCallId: callId,
            kind: "tool",
            rawType: "toolCall",
            toolName,
            label: summarizeToolArgs(toolName, args),
            subtitle: `${callId.slice(0, 12)} · pending · L${lineNumber}`,
            timestamp: msgTs,
            model,
            usage,
            toolInput: inputText,
            toolArgs: args,
            previewBody: ["── Input ──", inputText || "(none)"].join("\n"),
          }, item);
          toolEntryByCallId.set(callId, created);
        }
      }

      const replyText = extractPlainText(msg.content);
      if (replyText) {
        const tokenTotal = usage
          ? usage.input + usage.output + usage.cacheRead + usage.cacheWrite
          : 0;
        const subParts = [
          model ? model.split("/").pop()! : "",
          tokenTotal > 0 ? formatTokensM(tokenTotal) : "",
          formatByteHint(replyText),
          `L${lineNumber}`,
        ].filter(Boolean);
        pushEntry({
          lineNumber,
          id: entryId,
          kind: "assistant",
          rawType: "assistant",
          label: `Agent · ${summarizeOneLine(replyText, 60)}`,
          subtitle: subParts.join(" · "),
          timestamp: msgTs,
          model,
          usage,
          previewBody: replyText,
        }, obj);
      }
      continue;
    }

    // Any other message role preserved as metadata.
    const rawJson = formatCompactJson(obj, 24_000);
    pushEntry({
      lineNumber,
      id: entryId,
      kind: "meta",
      rawType: role || rawType,
      label: `${role || rawType} · L${lineNumber}`,
      subtitle: `L${lineNumber}`,
      timestamp: msgTs,
      previewBody: rawJson,
      rawJson,
    });
  }

  if (stat) {
    if (traceParseCache.size >= 2) traceParseCache.clear();
    traceParseCache.set(jsonlPath, { mtimeMs: stat.mtimeMs, size: stat.size, entries });
  }
  return entries;
}

/** Count trace entries per kind for header filter pills. */
export function countTraceKinds(
  entries: readonly TraceEntry[],
): Record<TraceFilter, number> {
  const counts: Record<TraceFilter, number> = {
    all: entries.length,
    tool: 0,
    user: 0,
    assistant: 0,
    thinking: 0,
    meta: 0,
  };
  for (const e of entries) {
    counts[e.kind] += 1;
  }
  return counts;
}

/** Filter trace entries by kind and multi-term search query. */
export function filterTraceEntries(
  entries: readonly TraceEntry[],
  options: { filter: TraceFilter; query: string },
): TraceEntry[] {
  const byKind =
    options.filter === "all"
      ? entries
      : entries.filter((entry) => entry.kind === options.filter);
  const trimmed = options.query.trim().toLowerCase();
  if (!trimmed) return [...byKind];
  const terms = trimmed.split(/\s+/).filter(Boolean);
  const matched: TraceEntry[] = [];
  for (const entry of byKind) {
    let ok = true;
    for (const term of terms) {
      if (!entry.searchLower.includes(term)) {
        ok = false;
        break;
      }
    }
    if (ok) matched.push(entry);
  }
  return matched;
}

export function formatTraceTime(ts: string): string {
  if (!ts) return "";
  const formatted = formatTimestamp(ts);
  return formatted.length >= 16 ? formatted.slice(11, 16) : formatted;
}
