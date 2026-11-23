/**
 * Data contract & preview loader for `extensions/pi-grok-config/`.
 *
 * Rust (`PiResourceCatalog`) is the single source of truth for resource
 * discovery, trust checks, and `settings.json` persistence. This module only
 * parses the Rust-generated JSON snapshot, filters rows for rendering, and
 * reads bounded file excerpts for the right-pane inspector.
 */

import fs from "node:fs";

export type ResourceTypeFilter = "All" | "Extensions" | "Skills" | "Prompts" | "Themes";
export type ResourceScopeView = "Global" | "Project";
export type ProjectOverrideMode = "Inherit" | "Load" | "Unload";

export interface PiConfigResource {
  id: string;
  displayName: string;
  path: string;
  resourceType: "Extensions" | "Skills" | "Prompts" | "Themes";
  scope: "Global" | "Project";
  origin: "auto" | "settings" | "package";
  source: string;
  enabled: boolean;
  inheritedEnabled: boolean;
  projectOverride: ProjectOverrideMode;
}

export interface PiConfigSnapshot {
  cwd: string;
  agentDir: string;
  projectTrusted: boolean;
  resources: PiConfigResource[];
}

export interface PiConfigMutationPayload {
  mode: "global" | "project";
  path: string;
  resourceType: PiConfigResource["resourceType"];
  enabled?: boolean;
  projectOverride?: ProjectOverrideMode;
}

const MAX_PREVIEW_BYTES = 48 * 1024;
const MAX_PREVIEW_LINES = 140;

export function loadPiConfigSnapshot(snapshotPath: string): PiConfigSnapshot | null {
  if (!snapshotPath || !fs.existsSync(snapshotPath)) return null;
  try {
    const raw = fs.readFileSync(snapshotPath, "utf8");
    const parsed = JSON.parse(raw) as Partial<PiConfigSnapshot>;
    if (!Array.isArray(parsed.resources)) return null;
    return {
      cwd: String(parsed.cwd ?? ""),
      agentDir: String(parsed.agentDir ?? ""),
      projectTrusted: Boolean(parsed.projectTrusted),
      resources: parsed.resources as PiConfigResource[],
    };
  } catch {
    return null;
  }
}

export function filterPiConfigResources(
  resources: readonly PiConfigResource[],
  typeFilter: ResourceTypeFilter,
  scopeView: ResourceScopeView,
  query: string,
): PiConfigResource[] {
  const terms = query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);

  return resources.filter((r) => {
    if (typeFilter !== "All" && r.resourceType !== typeFilter) return false;
    // In Global view, show user-scoped resources; in Project view, show all resources
    // (project-local + global resources eligible for project overrides).
    if (scopeView === "Global" && r.scope !== "Global") return false;
    if (terms.length === 0) return true;
    const hay = `${r.displayName} ${r.resourceType} ${r.origin} ${r.source} ${r.path}`.toLowerCase();
    return terms.every((t) => hay.includes(t));
  });
}

export function nextProjectOverride(cur: ProjectOverrideMode): ProjectOverrideMode {
  if (cur === "Inherit") return "Load";
  if (cur === "Load") return "Unload";
  return "Inherit";
}

export function effectiveEnabled(resource: PiConfigResource, scopeView: ResourceScopeView): boolean {
  if (scopeView === "Project") {
    if (resource.projectOverride === "Load") return true;
    if (resource.projectOverride === "Unload") return false;
    return resource.inheritedEnabled;
  }
  return resource.enabled;
}

export function readResourceFilePreview(filePath: string): string[] {
  if (!filePath || !fs.existsSync(filePath)) {
    return ["(file not found on disk)"];
  }
  try {
    const stat = fs.statSync(filePath);
    if (!stat.isFile()) return ["(directory)"];
    const readLen = Math.min(stat.size, MAX_PREVIEW_BYTES);
    const buf = Buffer.alloc(readLen);
    const fd = fs.openSync(filePath, "r");
    try {
      fs.readSync(fd, buf, 0, readLen, 0);
    } finally {
      fs.closeSync(fd);
    }
    const all = buf.toString("utf8").split(/\r?\n/);
    const lines = all.slice(0, MAX_PREVIEW_LINES);
    if (all.length > MAX_PREVIEW_LINES || stat.size > MAX_PREVIEW_BYTES) {
      lines.push("… preview truncated");
    }
    return lines;
  } catch {
    return ["(unable to read file preview)"];
  }
}
