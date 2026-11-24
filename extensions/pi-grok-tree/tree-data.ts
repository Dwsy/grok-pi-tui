/**
 * Session JSONL tree parser and DFS flattener for the TS `/tree` overlay.
 */

import fs from "node:fs";

/** Matches Rust `SessionTreeFilter` (`views/session_tree.rs`). */
export type TreeFilterMode = "default" | "no-tools" | "user-only" | "labeled" | "all";

export interface SessionTreeEntry {
  id: string;
  parentId: string | null;
  kind: "user" | "assistant" | "tool" | "branch" | "system";
  roleLabel: string;
  timestamp: string;
  summary: string;
  fullText: string;
  label?: string;
}

export interface FlattenedTreeNode {
  entry: SessionTreeEntry;
  depth: number;
  prefix: string;
  isOnActivePath: boolean;
  isLeaf: boolean;
  /** True when this node forks into 2+ children or is itself a sibling of a fork. */
  isBranchPoint: boolean;
  searchText: string;
}

function extractTextFromContent(content: unknown): string {
  if (typeof content === "string") return content.trim();
  if (!Array.isArray(content)) return "";
  const parts: string[] = [];
  for (const block of content) {
    if (!block || typeof block !== "object") continue;
    const rec = block as { type?: string; text?: string; name?: string };
    if (rec.type === "text" && typeof rec.text === "string" && rec.text.trim()) {
      parts.push(rec.text.trim());
    } else if (rec.type === "toolCall" && rec.name) {
      parts.push(`[tool: ${rec.name}]`);
    }
  }
  return parts.join("\n").trim();
}

function formatTimeShort(ts?: string): string {
  if (!ts) return "";
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts.slice(0, 16);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Parse a Pi session JSONL file into structured entries + labels + active leaf ID. */
export function parseSessionTreeJsonl(
  jsonlPath: string,
  explicitLeafId?: string,
): { entries: SessionTreeEntry[]; leafId: string | null } {
  if (!jsonlPath || !fs.existsSync(jsonlPath)) {
    return { entries: [], leafId: null };
  }

  let raw = "";
  try {
    raw = fs.readFileSync(jsonlPath, "utf8");
  } catch {
    return { entries: [], leafId: null };
  }

  const labels = new Map<string, string>();
  const entries: SessionTreeEntry[] = [];
  let lastEntryId: string | null = null;

  for (const line of raw.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    let obj: Record<string, unknown>;
    try {
      obj = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      continue;
    }

    const type = String(obj.type ?? "");
    if (type === "session") continue;

    if (type === "label") {
      const targetId = typeof obj.targetId === "string" ? obj.targetId : "";
      const label = typeof obj.label === "string" ? obj.label.trim() : "";
      if (targetId) {
        if (label) labels.set(targetId, label);
        else labels.delete(targetId);
      }
      continue;
    }

    const id = typeof obj.id === "string" ? obj.id : "";
    if (!id) continue;
    lastEntryId = id;
    const parentId = typeof obj.parentId === "string" ? obj.parentId : null;
    const timestamp = formatTimeShort(typeof obj.timestamp === "string" ? obj.timestamp : "");

    if (type === "message" && obj.message && typeof obj.message === "object") {
      const msg = obj.message as { role?: string; content?: unknown; toolName?: string };
      const role = String(msg.role ?? "");
      const fullText = extractTextFromContent(msg.content);
      const firstLine = fullText.replace(/\s+/g, " ").trim();

      if (role === "user") {
        entries.push({
          id,
          parentId,
          kind: "user",
          roleLabel: "You",
          timestamp,
          summary: firstLine || "(empty user message)",
          fullText: fullText || "(empty user message)",
        });
      } else if (role === "assistant") {
        entries.push({
          id,
          parentId,
          kind: "assistant",
          roleLabel: "Agent",
          timestamp,
          summary: firstLine || "(assistant turn)",
          fullText: fullText || "(assistant turn)",
        });
      } else if (role === "toolResult" || role === "tool") {
        const toolName = msg.toolName || "tool";
        entries.push({
          id,
          parentId,
          kind: "tool",
          roleLabel: toolName,
          timestamp,
          summary: firstLine ? `[${toolName}] ${firstLine}` : `[${toolName}] result`,
          fullText: fullText || `[${toolName}] result`,
        });
      } else {
        entries.push({
          id,
          parentId,
          kind: "system",
          roleLabel: role || "msg",
          timestamp,
          summary: firstLine || `(${role || "message"})`,
          fullText: fullText || `(${role || "message"})`,
        });
      }
    } else if (type === "branch_summary" || type === "compaction") {
      const summaryText = typeof obj.summary === "string" ? obj.summary.trim() : "";
      entries.push({
        id,
        parentId,
        kind: "branch",
        roleLabel: type === "compaction" ? "Compact" : "Branch",
        timestamp,
        summary: summaryText.replace(/\s+/g, " ") || `[${type}]`,
        fullText: summaryText || `[${type}]`,
      });
    } else {
      entries.push({
        id,
        parentId,
        kind: "system",
        roleLabel: type || "entry",
        timestamp,
        summary: `[${type || "entry"}]`,
        fullText: JSON.stringify(obj, null, 2),
      });
    }
  }

  for (const e of entries) {
    const lbl = labels.get(e.id);
    if (lbl) e.label = lbl;
  }

  return { entries, leafId: explicitLeafId || lastEntryId };
}

/** Build a DFS-ordered, box-drawn flattened tree ready for display & filtering. */
export function buildFlattenedSessionTree(
  entries: readonly SessionTreeEntry[],
  leafId: string | null,
): FlattenedTreeNode[] {
  const byId = new Map<string, SessionTreeEntry>();
  const children = new Map<string | null, SessionTreeEntry[]>();
  for (const e of entries) {
    byId.set(e.id, e);
  }
  for (const e of entries) {
    const pKey = e.parentId && byId.has(e.parentId) ? e.parentId : null;
    const list = children.get(pKey) ?? [];
    list.push(e);
    children.set(pKey, list);
  }

  // Trace active root -> leaf path.
  const activePath = new Set<string>();
  let cur = leafId;
  while (cur && byId.has(cur) && !activePath.has(cur)) {
    activePath.add(cur);
    cur = byId.get(cur)!.parentId;
  }

  const out: FlattenedTreeNode[] = [];
  const visit = (parentId: string | null, depth: number, ancestorGuides: string[]) => {
    const kids = children.get(parentId) ?? [];
    const isFork = parentId !== null && kids.length > 1;
    for (let i = 0; i < kids.length; i++) {
      const entry = kids[i]!;
      const isLast = i === kids.length - 1;
      const entryKids = children.get(entry.id) ?? [];
      // Linear single-child chains stay at the same indent level instead of
      // forming a diagonal staircase; only fork points add branch connectors
      // and increase depth (matching upstream pi-main Session Tree).
      const branch = isFork ? (isLast ? "└─ " : "├─ ") : "";
      const prefix = `${ancestorGuides.join("")}${branch}`;
      const isOnActivePath = activePath.has(entry.id);
      const isLeaf = entry.id === leafId;
      const isBranchPoint = isFork || entryKids.length > 1;
      out.push({
        entry,
        depth,
        prefix,
        isOnActivePath,
        isLeaf,
        isBranchPoint,
        searchText: `${entry.id} ${entry.roleLabel} ${entry.label ?? ""} ${entry.summary} ${entry.fullText}`.toLowerCase(),
      });
      const nextDepth = isFork ? depth + 1 : depth;
      const nextGuides = isFork
        ? [...ancestorGuides, isLast ? "   " : "│  "]
        : ancestorGuides;
      visit(entry.id, nextDepth, nextGuides);
    }
  };

  visit(null, 0, []);
  return out;
}

export function filterTreeNodes(
  nodes: readonly FlattenedTreeNode[],
  mode: TreeFilterMode,
  query: string,
): FlattenedTreeNode[] {
  const scoped = nodes.filter((n) => {
    if (mode === "all") return true;
    if (mode === "labeled") return Boolean(n.entry.label);
    if (mode === "user-only") return n.entry.kind === "user";
    if (mode === "no-tools") {
      return n.entry.kind !== "tool" && n.entry.kind !== "system";
    }
    // "default": all non-settings entries (matches Rust SessionTreeFilter::Default)
    return n.entry.kind !== "system";
  });

  const terms = query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);
  if (terms.length === 0) return scoped;
  return scoped.filter((n) => terms.every((t) => n.searchText.includes(t)));
}
