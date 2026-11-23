/**
 * High-performance file explorer data layer for the `@file` TS picker TUI.
 *
 * Inspired by NvChad (`nvim-tree.lua` + `nvim-web-devicons` + `telescope.nvim`):
 * - Nerd Font devicon glyphs with automatic terminal detection & clean fallback
 * - Lazy-expanded workspace directory tree (`fs.readdirSync` on demand)
 * - Smart file search backed by `fd` (with cached in-memory fuzzy ranking & fs fallback)
 * - Full-text search backed by `rg` (`--vimgrep --smart-case`, literal fallback on partial regex)
 * - Fast bounded file & directory preview with binary detection and line numbers
 */

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import {
  detectNerdFontSupport,
  getFileIcon,
  type FileIconSpec,
} from "../pi-grok-tui-kit/icons.ts";
import { formatTimestamp } from "./session-data.ts";
import { formatFileSize } from "./skill-data.ts";

export { detectNerdFontSupport, getFileIcon, type FileIconSpec };

export type FilePickerMode = "tree" | "files" | "grep";

export interface TreeNode {
  relPath: string;
  name: string;
  isDir: boolean;
  isSymlink: boolean;
  depth: number;
  expanded: boolean;
  isLast: boolean;
  /** True for each ancestor level that still has following siblings (for `│` guides). */
  ancestorHasNext: boolean[];
}

export interface FileSearchEntry {
  relPath: string;
  name: string;
  isDir: boolean;
}

export interface GrepHit {
  relPath: string;
  line: number;
  column: number;
  text: string;
}

export interface FilePreviewResult {
  kind: "file" | "dir" | "binary" | "error";
  relPath: string;
  sizeLabel: string;
  modified: string;
  lineCount: number;
  truncated: boolean;
  lines: string[];
  dirSummary?: string;
}

const MAX_FD_RESULTS = 2000;
const MAX_RG_RESULTS = 200;
const MAX_PREVIEW_BYTES = 128 * 1024;
const MAX_PREVIEW_LINES = 500;
const IGNORED_TREE_DIRS = new Set([".git", "node_modules", "target", ".next", "dist", "build"]);

// Icon helpers are imported and re-exported from ../pi-grok-tui-kit/icons.ts.

// ============================================================================
// Lazy directory tree builder
// ============================================================================

function listDirSorted(
  absDir: string,
  showHidden: boolean,
): Array<{ name: string; isDir: boolean; isSymlink: boolean }> {
  try {
    const entries = fs.readdirSync(absDir, { withFileTypes: true });
    const out: Array<{ name: string; isDir: boolean; isSymlink: boolean }> = [];
    for (const entry of entries) {
      if (!showHidden && entry.name.startsWith(".")) continue;
      if (entry.isDirectory() && IGNORED_TREE_DIRS.has(entry.name) && !showHidden) continue;
      let isDir = entry.isDirectory();
      const isSymlink = entry.isSymbolicLink();
      if (isSymlink) {
        try {
          isDir = fs.statSync(path.join(absDir, entry.name)).isDirectory();
        } catch {
          isDir = false;
        }
      }
      out.push({ name: entry.name, isDir, isSymlink });
    }
    out.sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    return out;
  } catch {
    return [];
  }
}

/**
 * Flatten the visible workspace directory tree given the currently expanded
 * relative directory paths. Only expanded directories are read from disk.
 */
export function buildVisibleTreeNodes(
  rootDir: string,
  expandedDirs: ReadonlySet<string>,
  options: { showHidden: boolean; maxDepth?: number } = { showHidden: false },
): TreeNode[] {
  const maxDepth = options.maxDepth ?? 8;
  const result: TreeNode[] = [];

  const walk = (relDir: string, depth: number, ancestorHasNext: boolean[]) => {
    if (depth > maxDepth) return;
    const absDir = relDir ? path.join(rootDir, relDir) : rootDir;
    const entries = listDirSorted(absDir, options.showHidden);
    for (let i = 0; i < entries.length; i++) {
      const entry = entries[i]!;
      const relPath = relDir ? `${relDir}/${entry.name}` : entry.name;
      const isLast = i === entries.length - 1;
      const expanded = entry.isDir && expandedDirs.has(relPath);
      result.push({
        relPath,
        name: entry.name,
        isDir: entry.isDir,
        isSymlink: entry.isSymlink,
        depth,
        expanded,
        isLast,
        ancestorHasNext,
      });
      if (expanded) {
        walk(relPath, depth + 1, [...ancestorHasNext, !isLast]);
      }
    }
  };

  walk("", 0, []);
  return result;
}

// ============================================================================
// High-performance `fd` smart file search (with Node fs fallback)
// ============================================================================

function walkFallbackFiles(
  rootDir: string,
  showHidden: boolean,
  limit: number,
): FileSearchEntry[] {
  const out: FileSearchEntry[] = [];
  const stack: Array<{ rel: string; depth: number }> = [{ rel: "", depth: 0 }];
  while (stack.length > 0 && out.length < limit) {
    const { rel, depth } = stack.pop()!;
    if (depth > 8) continue;
    const abs = rel ? path.join(rootDir, rel) : rootDir;
    const entries = listDirSorted(abs, showHidden);
    for (const entry of entries) {
      const nextRel = rel ? `${rel}/${entry.name}` : entry.name;
      out.push({ relPath: nextRel, name: entry.name, isDir: entry.isDir });
      if (out.length >= limit) break;
      if (entry.isDir) stack.push({ rel: nextRel, depth: depth + 1 });
    }
  }
  return out;
}

/**
 * List workspace files & directories using `fd` (respecting `.gitignore` by
 * default, optionally including hidden files). Falls back to bounded Node fs
 * walk if `fd` is not installed.
 */
export function listWorkspaceFilesWithFd(
  rootDir: string,
  options: { showHidden: boolean; limit?: number } = { showHidden: false },
): FileSearchEntry[] {
  const limit = options.limit ?? MAX_FD_RESULTS;
  const args = [
    "--color=never",
    "--strip-cwd-prefix",
    "--max-results",
    String(limit),
    "--exclude",
    ".git",
  ];
  if (options.showHidden) {
    args.push("--hidden");
  }

  try {
    const proc = spawnSync("fd", args, {
      cwd: rootDir,
      encoding: "utf8",
      maxBuffer: 8 * 1024 * 1024,
      timeout: 3000,
    });
    if (proc.status === 0 && typeof proc.stdout === "string") {
      const out: FileSearchEntry[] = [];
      for (const rawLine of proc.stdout.split("\n")) {
        const line = rawLine.trim();
        if (!line) continue;
        const isDir = line.endsWith("/");
        const relPath = isDir ? line.slice(0, -1) : line;
        if (!relPath) continue;
        const name = path.posix.basename(relPath);
        out.push({ relPath, name, isDir });
      }
      return out;
    }
  } catch {
    // Fall back to Node fs walk below.
  }

  return walkFallbackFiles(rootDir, options.showHidden, limit);
}

function fuzzySubsequenceScore(haystack: string, needle: string): number | null {
  if (!needle) return 0;
  if (haystack === needle) return 1000;
  if (haystack.startsWith(needle)) return 800 - Math.min(150, haystack.length - needle.length);
  const subIdx = haystack.indexOf(needle);
  if (subIdx >= 0) {
    const boundary = subIdx === 0 || /[-_/.:\s]/.test(haystack[subIdx - 1] ?? "");
    return Math.max(300, 600 + (boundary ? 40 : 0) - subIdx * 2);
  }
  let qi = 0;
  let score = 200;
  let prevMatch = -2;
  for (let i = 0; i < haystack.length && qi < needle.length; i++) {
    if (haystack[i] === needle[qi]) {
      if (i === prevMatch + 1) score += 18;
      else score -= 2;
      if (i === 0 || /[-_/.:\s]/.test(haystack[i - 1] ?? "")) score += 24;
      prevMatch = i;
      qi++;
    }
  }
  return qi === needle.length ? Math.max(50, score) : null;
}

/** Smart multi-term ranking over `fd` entries (filename matches outrank path matches). */
export function filterFileEntries(
  entries: readonly FileSearchEntry[],
  query: string,
  limit = 200,
): FileSearchEntry[] {
  const trimmed = query.trim().toLowerCase();
  if (!trimmed) {
    return entries.slice(0, limit);
  }
  const terms = trimmed.split(/\s+/).filter(Boolean);
  const scored: Array<{ score: number; index: number; entry: FileSearchEntry }> = [];

  for (let i = 0; i < entries.length; i++) {
    const entry = entries[i]!;
    const nameLower = entry.name.toLowerCase();
    const pathLower = entry.relPath.toLowerCase();
    let total = 0;
    let ok = true;
    for (const term of terms) {
      const nameScore = fuzzySubsequenceScore(nameLower, term);
      const pathScore = fuzzySubsequenceScore(pathLower, term);
      if (nameScore === null && pathScore === null) {
        ok = false;
        break;
      }
      total += Math.max(nameScore !== null ? nameScore + 80 : 0, pathScore ?? 0);
    }
    if (ok) {
      // Prefer files slightly over dirs on equal score, and shorter paths.
      const bonus = (entry.isDir ? 0 : 10) - Math.min(40, Math.floor(entry.relPath.length / 4));
      scored.push({ score: total + bonus, index: i, entry });
    }
  }

  scored.sort((a, b) => b.score - a.score || a.index - b.index);
  return scored.slice(0, limit).map((s) => s.entry);
}

// ============================================================================
// High-performance `rg` full-text search (Telescope live_grep style)
// ============================================================================

export function parseRgVimgrepOutput(stdout: string, limit = MAX_RG_RESULTS): GrepHit[] {
  const hits: GrepHit[] = [];
  for (const rawLine of stdout.split("\n")) {
    if (!rawLine) continue;
    const match = rawLine.match(/^(.+?):(\d+):(\d+):(.*)$/);
    if (!match) continue;
    hits.push({
      relPath: match[1]!.replace(/^\.\//, ""),
      line: Number(match[2]!),
      column: Number(match[3]!),
      text: match[4]!.trimEnd(),
    });
    if (hits.length >= limit) break;
  }
  return hits;
}

/**
 * Run `rg --vimgrep --smart-case` across the workspace. Automatically retries
 * with `--fixed-strings` when the user is mid-typing an incomplete regex.
 */
export function grepWorkspaceWithRg(
  rootDir: string,
  query: string,
  options: { showHidden: boolean; limit?: number } = { showHidden: false },
): GrepHit[] {
  const trimmed = query.trim();
  if (!trimmed) return [];
  const limit = options.limit ?? MAX_RG_RESULTS;

  const runRg = (fixedStrings: boolean) => {
    const args = [
      "--vimgrep",
      "--no-heading",
      "--color=never",
      "--smart-case",
      "--max-count",
      "6",
      "--max-columns",
      "240",
      "--max-columns-preview",
      "--glob",
      "!.git/**",
    ];
    if (options.showHidden) args.push("--hidden");
    if (fixedStrings) args.push("--fixed-strings");
    args.push("--", trimmed, ".");
    return spawnSync("rg", args, {
      cwd: rootDir,
      encoding: "utf8",
      maxBuffer: 4 * 1024 * 1024,
      timeout: 2500,
    });
  };

  try {
    let proc = runRg(false);
    // rg exit code 2 indicates regex parse error; retry literally so typing `foo(` works.
    if (proc.status === 2) {
      proc = runRg(true);
    }
    if ((proc.status === 0 || proc.status === 1) && typeof proc.stdout === "string") {
      return parseRgVimgrepOutput(proc.stdout, limit);
    }
  } catch {
    // rg unavailable.
  }
  return [];
}

// ============================================================================
// File & directory preview loader
// ============================================================================

export function loadFileOrDirPreview(
  rootDir: string,
  relPath: string,
  options: { showHidden: boolean; nerdIcons: boolean },
): FilePreviewResult {
  const abs = path.resolve(rootDir, relPath);
  try {
    const stat = fs.statSync(abs);
    const modified = formatTimestamp(stat.mtime.toISOString());

    if (stat.isDirectory()) {
      const entries = listDirSorted(abs, options.showHidden);
      const dirs = entries.filter((e) => e.isDir);
      const files = entries.filter((e) => !e.isDir);
      const lines = entries.slice(0, 120).map((e) => {
        const icon = getFileIcon(e.name, {
          isDir: e.isDir,
          isSymlink: e.isSymlink,
          nerdIcons: options.nerdIcons,
        });
        return `${icon.glyph} ${e.name}${e.isDir ? "/" : ""}`;
      });
      if (entries.length > 120) {
        lines.push(`… ${entries.length - 120} more entries`);
      }
      return {
        kind: "dir",
        relPath,
        sizeLabel: `${entries.length} items`,
        modified,
        lineCount: entries.length,
        truncated: entries.length > 120,
        lines,
        dirSummary: `${dirs.length} dirs · ${files.length} files`,
      };
    }

    const readLen = Math.min(stat.size, MAX_PREVIEW_BYTES);
    const buf = Buffer.alloc(readLen);
    const fd = fs.openSync(abs, "r");
    try {
      fs.readSync(fd, buf, 0, readLen, 0);
    } finally {
      fs.closeSync(fd);
    }

    // Check first 4 KB for NUL byte to detect binary files cleanly.
    const probeLen = Math.min(readLen, 4096);
    for (let i = 0; i < probeLen; i++) {
      if (buf[i] === 0) {
        return {
          kind: "binary",
          relPath,
          sizeLabel: formatFileSize(stat.size),
          modified,
          lineCount: 0,
          truncated: false,
          lines: [`Binary file (${formatFileSize(stat.size)}) — preview skipped.`],
        };
      }
    }

    const text = buf.toString("utf8");
    const allLines = text.split(/\r?\n/);
    const truncated = stat.size > MAX_PREVIEW_BYTES || allLines.length > MAX_PREVIEW_LINES;
    const lines = allLines.slice(0, MAX_PREVIEW_LINES);
    return {
      kind: "file",
      relPath,
      sizeLabel: formatFileSize(stat.size),
      modified,
      lineCount: allLines.length,
      truncated,
      lines,
    };
  } catch (err) {
    return {
      kind: "error",
      relPath,
      sizeLabel: "—",
      modified: "—",
      lineCount: 0,
      truncated: false,
      lines: [`Unable to read ${relPath}: ${err instanceof Error ? err.message : String(err)}`],
    };
  }
}
