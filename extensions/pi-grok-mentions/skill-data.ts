/**
 * Skill data normalization, scope classification, search, and lazy `SKILL.md`
 * preview loader for the `@skill` TS picker TUI.
 *
 * Skill entries come directly from Pi's live skill registry (zero startup disk
 * scanning); `SKILL.md` files are read lazily and cached only when a skill is
 * selected for preview.
 */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { formatTimestamp } from "./session-data.ts";

export type SkillScope = "global" | "project";

export interface SkillRecord {
  id: string;
  title: string;
  description: string;
  sourcePath?: string;
  /** Whether this skill lives inside the active workspace (`project`) or outside (`global`). */
  origin: SkillScope;
  /** Shortened human-readable path (`./...` or `~/...`). */
  displayPath: string;
}

export interface SkillPreviewDetail {
  frontmatter: Record<string, string>;
  body: string;
  lineCount: number;
  sizeBytes: number;
  modified: string;
  bundledDirs: string[];
}

export interface SkillSearchHit {
  record: SkillRecord;
}

/** Check whether `skillPath` resides inside `currentCwd`. */
export function isProjectSkillPath(skillPath: string | undefined, currentCwd: string): boolean {
  if (!skillPath || !currentCwd) return false;
  const normSkill = path.resolve(skillPath);
  const normCwd = path.resolve(currentCwd).replace(/[\\/]+$/, "");
  return normSkill === normCwd || normSkill.startsWith(`${normCwd}${path.sep}`);
}

/** Shorten an absolute path relative to `currentCwd` or `$HOME` for compact display. */
export function formatDisplayPath(skillPath: string | undefined, currentCwd: string): string {
  if (!skillPath) return "—";
  const normSkill = path.resolve(skillPath);
  if (currentCwd) {
    const normCwd = path.resolve(currentCwd).replace(/[\\/]+$/, "");
    if (normSkill.startsWith(`${normCwd}${path.sep}`)) {
      return `./${normSkill.slice(normCwd.length + 1)}`;
    }
  }
  const home = os.homedir();
  if (home && normSkill.startsWith(`${home}${path.sep}`)) {
    return `~/${normSkill.slice(home.length + 1)}`;
  }
  return normSkill;
}

/** Format byte size into a compact KB/B label. */
export function formatFileSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} KB`;
}

/** Normalize raw skill items collected from Pi into sorted `SkillRecord`s. */
export function buildSkillRecords(
  items: ReadonlyArray<{ id: string; title: string; subtitle: string; sourcePath?: string }>,
  currentCwd: string,
): SkillRecord[] {
  return items
    .map((item) => {
      const origin: SkillScope = isProjectSkillPath(item.sourcePath, currentCwd)
        ? "project"
        : "global";
      return {
        id: item.id,
        title: item.title || item.id,
        description: item.subtitle || "",
        sourcePath: item.sourcePath,
        origin,
        displayPath: formatDisplayPath(item.sourcePath, currentCwd),
      };
    })
    .sort((a, b) => {
      // Project skills first within the same alphabetical tier when browsing globally,
      // then alphabetical by id for predictable lookup.
      if (a.origin !== b.origin) return a.origin === "project" ? -1 : 1;
      return a.id.localeCompare(b.id);
    });
}

/**
 * Parse lightweight YAML frontmatter (`--- ... ---`) from a `SKILL.md` file
 * without pulling in an external YAML parser.
 */
export function parseSkillMarkdown(raw: string): {
  frontmatter: Record<string, string>;
  body: string;
} {
  const normalized = raw.replace(/\r\n/g, "\n");
  const match = normalized.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
  if (!match) {
    return { frontmatter: {}, body: normalized.trim() };
  }
  const frontmatter: Record<string, string> = {};
  for (const line of match[1]!.split("\n")) {
    const kv = line.match(/^([A-Za-z0-9_-]+)\s*:\s*(.*)$/);
    if (!kv) continue;
    const key = kv[1]!.trim();
    const val = kv[2]!.trim().replace(/^["']|["']$/g, "");
    if (key && val) frontmatter[key] = val;
  }
  return { frontmatter, body: match[2]!.trim() };
}

/**
 * Lazily load `SKILL.md` contents, file stats, and companion resource folders
 * (`scripts`, `references`, `assets`) for the currently previewed skill.
 */
export function loadSkillPreviewDetail(sourcePath: string | undefined): SkillPreviewDetail | null {
  if (!sourcePath || !fs.existsSync(sourcePath)) return null;
  try {
    const stat = fs.statSync(sourcePath);
    if (!stat.isFile()) return null;
    const raw = fs.readFileSync(sourcePath, "utf8");
    const { frontmatter, body } = parseSkillMarkdown(raw);
    const lineCount = raw.length === 0 ? 0 : raw.split(/\r?\n/).length;

    const bundledDirs: string[] = [];
    const parentDir = path.dirname(sourcePath);
    for (const sub of ["scripts", "references", "assets"]) {
      const candidate = path.join(parentDir, sub);
      try {
        if (fs.existsSync(candidate) && fs.statSync(candidate).isDirectory()) {
          const count = fs.readdirSync(candidate).filter((n) => !n.startsWith(".")).length;
          if (count > 0) bundledDirs.push(`${sub}(${count})`);
        }
      } catch {
        // Ignore unreadable companion directory.
      }
    }

    return {
      frontmatter,
      body,
      lineCount,
      sizeBytes: stat.size,
      modified: formatTimestamp(stat.mtime.toISOString()),
      bundledDirs,
    };
  } catch {
    return null;
  }
}

/**
 * Filter and rank skills by scope (`global` = all skills, `project` = workspace
 * skills only) and multi-term search query across id, title, description, path,
 * and optional cached preview text.
 */
export function filterSkillRecords(
  records: readonly SkillRecord[],
  options: {
    scope: SkillScope;
    query: string;
    bodyIndex?: ReadonlyMap<string, string>;
  },
): SkillSearchHit[] {
  const scoped =
    options.scope === "project" ? records.filter((r) => r.origin === "project") : records;

  const trimmed = options.query.trim().toLowerCase();
  if (!trimmed) {
    return scoped.map((record) => ({ record }));
  }

  const terms = trimmed.split(/\s+/).filter(Boolean);
  const scored: Array<{ score: number; index: number; record: SkillRecord }> = [];

  for (let i = 0; i < scoped.length; i++) {
    const record = scoped[i]!;
    const idLower = record.id.toLowerCase();
    const descLower = record.description.toLowerCase();
    const pathLower = record.displayPath.toLowerCase();
    const bodyLower = (options.bodyIndex?.get(record.id) ?? "").toLowerCase();

    let score = 0;
    let matchedAll = true;

    for (const term of terms) {
      const exactId = idLower === term;
      const prefixId = idLower.startsWith(term);
      const inId = idLower.includes(term);
      const inDesc = descLower.includes(term);
      const inPath = pathLower.includes(term);
      const inBody = bodyLower.includes(term);

      if (!inId && !inDesc && !inPath && !inBody) {
        matchedAll = false;
        break;
      }
      if (exactId) score += 200;
      else if (prefixId) score += 140;
      else if (inId) score += 100;
      if (inDesc) score += 60;
      if (inPath) score += 30;
      if (inBody) score += 20;
    }

    if (matchedAll) {
      scored.push({ score, index: i, record });
    }
  }

  scored.sort((a, b) => b.score - a.score || a.index - b.index);
  return scored.map((entry) => ({ record: entry.record }));
}
