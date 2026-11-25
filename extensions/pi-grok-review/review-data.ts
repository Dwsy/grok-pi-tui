/**
 * Session & message code-review data extractor for `pi-grok-review`.
 *
 * Extracts `edit` / `write` tool calls from Pi's active session JSONL grouped
 * by user turn, computes compact unified diff lines, and falls back to `git diff`
 * when inspecting a workspace without session tool edits.
 */

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

export type ReviewDiffKind = "context" | "add" | "del" | "hunk";

export interface ReviewDiffLine {
  kind: ReviewDiffKind;
  oldLine?: number;
  newLine?: number;
  text: string;
}

export interface ReviewFileChange {
  relPath: string;
  name: string;
  op: "edit" | "write";
  opsCount: number;
  additions: number;
  deletions: number;
  diffLines: ReviewDiffLine[];
}

export interface ReviewTurn {
  turnIndex: number;
  prompt: string;
  timestamp: string;
  files: ReviewFileChange[];
}

export interface ReviewTreeRow {
  kind: "dir" | "file";
  relPath: string;
  name: string;
  depth: number;
  isLast: boolean;
  ancestorHasNext: boolean[];
  fileIndex?: number;
}

function normalizeRelPath(rawPath: string, cwd: string): string {
  const trimmed = rawPath.trim();
  if (!trimmed) return "";
  if (path.isAbsolute(trimmed)) {
    const rel = path.relative(cwd, trimmed);
    if (rel && !rel.startsWith("..")) return rel.replace(/\\/g, "/");
  }
  return trimmed.replace(/^\.\//, "").replace(/\\/g, "/");
}

function extractPlainUserText(content: unknown): string {
  if (typeof content === "string") return content.replace(/\s+/g, " ").trim();
  if (!Array.isArray(content)) return "";
  const parts: string[] = [];
  for (const item of content) {
    if (item && typeof item === "object" && (item as { type?: string }).type === "text") {
      const text = (item as { text?: unknown }).text;
      if (typeof text === "string" && text.trim()) parts.push(text.trim());
    }
  }
  return parts.join(" ").replace(/\s+/g, " ").trim();
}

/** Build a compact hunk diff between `oldText` and `newText` for an `edit` tool call. */
export function buildEditDiffLines(oldText: string, newText: string): {
  lines: ReviewDiffLine[];
  additions: number;
  deletions: number;
} {
  const oldArr = oldText ? oldText.replace(/\r\n/g, "\n").split("\n") : [];
  const newArr = newText ? newText.replace(/\r\n/g, "\n").split("\n") : [];

  // Trim shared prefix/suffix so only the actual changed region + 2 lines of context show.
  let prefix = 0;
  while (prefix < oldArr.length && prefix < newArr.length && oldArr[prefix] === newArr[prefix]) {
    prefix++;
  }
  let suffix = 0;
  while (
    suffix < oldArr.length - prefix &&
    suffix < newArr.length - prefix &&
    oldArr[oldArr.length - 1 - suffix] === newArr[newArr.length - 1 - suffix]
  ) {
    suffix++;
  }

  const ctxBeforeStart = Math.max(0, prefix - 2);
  const ctxAfterEndOld = Math.min(oldArr.length, oldArr.length - suffix + 2);
  const lines: ReviewDiffLine[] = [
    {
      kind: "hunk",
      text: `@@ -${ctxBeforeStart + 1},${oldArr.length - suffix - ctxBeforeStart} +${ctxBeforeStart + 1},${newArr.length - suffix - ctxBeforeStart} @@`,
    },
  ];

  for (let i = ctxBeforeStart; i < prefix; i++) {
    lines.push({ kind: "context", oldLine: i + 1, newLine: i + 1, text: oldArr[i]! });
  }
  let deletions = 0;
  for (let i = prefix; i < oldArr.length - suffix; i++) {
    lines.push({ kind: "del", oldLine: i + 1, text: oldArr[i]! });
    deletions++;
  }
  let additions = 0;
  for (let i = prefix; i < newArr.length - suffix; i++) {
    lines.push({ kind: "add", newLine: i + 1, text: newArr[i]! });
    additions++;
  }
  const newAfterStart = newArr.length - suffix;
  for (let i = 0; i < ctxAfterEndOld - (oldArr.length - suffix); i++) {
    const oldIdx = oldArr.length - suffix + i;
    const newIdx = newAfterStart + i;
    lines.push({ kind: "context", oldLine: oldIdx + 1, newLine: newIdx + 1, text: oldArr[oldIdx]! });
  }

  return { lines, additions, deletions };
}

/** Build diff lines for a `write` tool call (capped at 300 lines for instant render). */
export function buildWriteDiffLines(content: string): {
  lines: ReviewDiffLine[];
  additions: number;
} {
  const arr = content ? content.replace(/\r\n/g, "\n").split("\n") : [];
  const shown = arr.slice(0, 300);
  const lines: ReviewDiffLine[] = [
    { kind: "hunk", text: `@@ -0,0 +1,${arr.length} @@ (write)` },
    ...shown.map((text, idx) => ({ kind: "add" as const, newLine: idx + 1, text })),
  ];
  if (arr.length > shown.length) {
    lines.push({ kind: "hunk", text: `… ${arr.length - shown.length} more lines` });
  }
  return { lines, additions: arr.length };
}

function mergeFileChange(
  map: Map<string, ReviewFileChange>,
  relPath: string,
  op: "edit" | "write",
  diffLines: ReviewDiffLine[],
  additions: number,
  deletions: number,
): void {
  const existing = map.get(relPath);
  if (!existing) {
    map.set(relPath, {
      relPath,
      name: path.posix.basename(relPath),
      op,
      opsCount: 1,
      additions,
      deletions,
      diffLines: [...diffLines],
    });
    return;
  }
  existing.opsCount += 1;
  existing.additions += additions;
  existing.deletions += deletions;
  if (op === "write") existing.op = "write";
  existing.diffLines.push(...diffLines);
}

/**
 * Extract turn-grouped `edit`/`write` file changes from a Pi session JSONL file.
 */
export function extractSessionReviewTurns(sessionJsonlPath: string, cwd: string): ReviewTurn[] {
  if (!sessionJsonlPath || !fs.existsSync(sessionJsonlPath)) return [];
  let raw: string;
  try {
    raw = fs.readFileSync(sessionJsonlPath, "utf8");
  } catch {
    return [];
  }

  const turns: Array<{ prompt: string; timestamp: string; files: Map<string, ReviewFileChange> }> =
    [];
  let current:
    | { prompt: string; timestamp: string; files: Map<string, ReviewFileChange> }
    | undefined;

  for (const line of raw.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    let obj: Record<string, unknown>;
    try {
      obj = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      continue;
    }
    if (obj.type && obj.type !== "message") continue;
    const msg = (obj.message ?? obj) as {
      role?: unknown;
      content?: unknown;
      timestamp?: unknown;
    };
    const role = typeof msg.role === "string" ? msg.role.toLowerCase() : "";
    const ts =
      typeof obj.timestamp === "string"
        ? obj.timestamp
        : typeof msg.timestamp === "string"
          ? msg.timestamp
          : "";

    if (role === "user") {
      const text = extractPlainUserText(msg.content);
      if (text) {
        current = { prompt: text, timestamp: ts, files: new Map() };
        turns.push(current);
      }
      continue;
    }

    if (role === "assistant" && Array.isArray(msg.content)) {
      for (const block of msg.content) {
        if (!block || typeof block !== "object") continue;
        const item = block as {
          type?: unknown;
          name?: unknown;
          arguments?: Record<string, unknown>;
          input?: Record<string, unknown>;
        };
        const btype = typeof item.type === "string" ? item.type.toLowerCase() : "";
        if (btype !== "toolcall" && btype !== "tool_call" && btype !== "tool_use") continue;
        const toolName = typeof item.name === "string" ? item.name.toLowerCase() : "";
        if (toolName !== "edit" && toolName !== "write") continue;
        const args = item.arguments ?? item.input ?? {};
        const rawFilePath = String(args.path ?? args.file_path ?? args.filePath ?? "");
        const relPath = normalizeRelPath(rawFilePath, cwd);
        if (!relPath) continue;

        if (!current) {
          current = { prompt: "(Session start)", timestamp: ts, files: new Map() };
          turns.push(current);
        }

        if (toolName === "edit") {
          const oldText = String(args.oldText ?? args.old_text ?? args.old_string ?? "");
          const newText = String(args.newText ?? args.new_text ?? args.new_string ?? "");
          const { lines, additions, deletions } = buildEditDiffLines(oldText, newText);
          mergeFileChange(current.files, relPath, "edit", lines, additions, deletions);
        } else {
          const content = String(args.content ?? args.text ?? "");
          const { lines, additions } = buildWriteDiffLines(content);
          mergeFileChange(current.files, relPath, "write", lines, additions, 0);
        }
      }
    }
  }

  return turns
    .filter((t) => t.files.size > 0)
    .map((t, idx) => ({
      turnIndex: idx + 1,
      prompt: t.prompt,
      timestamp: t.timestamp,
      files: [...t.files.values()].sort((a, b) => a.relPath.localeCompare(b.relPath)),
    }));
}

/** Merge all turns into a single session-level changed file list. */
export function aggregateSessionFiles(turns: readonly ReviewTurn[]): ReviewFileChange[] {
  const merged = new Map<string, ReviewFileChange>();
  for (const turn of turns) {
    for (const file of turn.files) {
      mergeFileChange(
        merged,
        file.relPath,
        file.op,
        file.diffLines,
        file.additions,
        file.deletions,
      );
    }
  }
  return [...merged.values()].sort((a, b) => a.relPath.localeCompare(b.relPath));
}

/** Fallback: load uncommitted workspace changes via `git diff` when session JSONL has no edits. */
export function loadGitWorkingTreeChanges(cwd: string): ReviewFileChange[] {
  try {
    const proc = spawnSync("git", ["diff", "--unified=3", "--no-color"], {
      cwd,
      encoding: "utf8",
      maxBuffer: 4 * 1024 * 1024,
      timeout: 2500,
    });
    if (proc.status !== 0 || !proc.stdout) return [];

    const files: ReviewFileChange[] = [];
    let current: ReviewFileChange | null = null;
    let oldNo = 0;
    let newNo = 0;

    for (const line of proc.stdout.split("\n")) {
      if (line.startsWith("diff --git ")) {
        const m = line.match(/^diff --git a\/(.+?) b\/(.+)$/);
        if (m) {
          const relPath = m[2]!;
          current = {
            relPath,
            name: path.posix.basename(relPath),
            op: "edit",
            opsCount: 1,
            additions: 0,
            deletions: 0,
            diffLines: [],
          };
          files.push(current);
        }
        continue;
      }
      if (!current) continue;
      if (line.startsWith("@@ ")) {
        const m = line.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
        oldNo = m ? Number(m[1]) : 1;
        newNo = m ? Number(m[2]) : 1;
        current.diffLines.push({ kind: "hunk", text: line });
      } else if (line.startsWith("+") && !line.startsWith("+++")) {
        current.additions += 1;
        current.diffLines.push({ kind: "add", newLine: newNo++, text: line.slice(1) });
      } else if (line.startsWith("-") && !line.startsWith("---")) {
        current.deletions += 1;
        current.diffLines.push({ kind: "del", oldLine: oldNo++, text: line.slice(1) });
      } else if (line.startsWith(" ")) {
        current.diffLines.push({
          kind: "context",
          oldLine: oldNo++,
          newLine: newNo++,
          text: line.slice(1),
        });
      }
    }
    return files.filter((f) => f.diffLines.length > 0);
  } catch {
    return [];
  }
}

/** Maximum number of path segments folded into one rendered directory node. */
const REVIEW_TREE_SEGMENTS_PER_NODE = 3;

/** Build a compact directory-tree view over the changed file list. */
export function buildReviewFileTree(files: readonly ReviewFileChange[]): ReviewTreeRow[] {
  interface Branch {
    name: string;
    relPath: string;
    dirs: Map<string, Branch>;
    files: Array<{ name: string; relPath: string; index: number }>;
  }

  const root: Branch = { name: "", relPath: "", dirs: new Map(), files: [] };
  for (let i = 0; i < files.length; i++) {
    const f = files[i]!;
    const parts = f.relPath.split("/").filter(Boolean);
    const dirParts = parts.slice(0, -1);
    let node = root;
    for (let d = 0; d < dirParts.length; d += REVIEW_TREE_SEGMENTS_PER_NODE) {
      const seg = dirParts
        .slice(d, d + REVIEW_TREE_SEGMENTS_PER_NODE)
        .join("/");
      const nextRel = node.relPath ? `${node.relPath}/${seg}` : seg;
      let child = node.dirs.get(seg);
      if (!child) {
        child = { name: seg, relPath: nextRel, dirs: new Map(), files: [] };
        node.dirs.set(seg, child);
      }
      node = child;
    }
    node.files.push({
      name: parts[parts.length - 1] ?? f.name,
      relPath: f.relPath,
      index: i,
    });
  }

  const rows: ReviewTreeRow[] = [];
  const walk = (branch: Branch, depth: number, ancestorHasNext: boolean[]) => {
    const dirList = [...branch.dirs.values()].sort((a, b) => a.name.localeCompare(b.name));
    const fileList = [...branch.files].sort((a, b) => a.name.localeCompare(b.name));
    const total = dirList.length + fileList.length;

    dirList.forEach((dir, idx) => {
      const isLast = idx === total - 1;
      rows.push({
        kind: "dir",
        relPath: dir.relPath,
        name: dir.name,
        depth,
        isLast,
        ancestorHasNext,
      });
      walk(dir, depth + 1, [...ancestorHasNext, !isLast]);
    });

    fileList.forEach((file, idx) => {
      const isLast = dirList.length + idx === total - 1;
      rows.push({
        kind: "file",
        relPath: file.relPath,
        name: file.name,
        depth,
        isLast,
        ancestorHasNext,
        fileIndex: file.index,
      });
    });
  };

  walk(root, 0, []);
  return rows;
}
