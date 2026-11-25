/**
 * Full bordered left/right split TUI for the `@session` mention picker.
 *
 * Renders inside `ctx.ui.custom(..., { overlay: true, overlayOptions })` via
 * `pi-grok-remote-tui` (and natively in standalone Pi TUI):
 * - Outer rounded box border (`╭─╮│╰─╯`) with vertical divider (`┬│┴`)
 * - Header bar with Global / Project scope toggle (defaults to Global) & `[✕]`
 * - Live search bar (searches metadata + User & Agent messages only)
 * - Left pane: scrollable session list (title, age, project, msgs, tokens in M, model)
 * - Right pane: full metadata card (ID, Project, Created, Updated, Msgs, Tokens in M, Model)
 *   + scrollable User & Agent message preview (tool calls & thinking hidden)
 * - Full keyboard + mouse interaction (click pills, click/double-click rows, wheel scroll)
 */

import {
  matchesKey,
  truncateToWidth,
  visibleWidth,
  type Component,
  type TuiMouseEvent,
  type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import os from "node:os";
import path from "node:path";
import {
  computeSplitGeometry,
  fitWidth,
  highlightTerms,
  hitRegion,
  RenderCoalescer,
  renderSplitOverlayFrame,
  wrapTextToWidth,
  type ClickRegion,
  type TuiTheme,
} from "../pi-grok-tui-kit/index.ts";
import {
  appendSessionNameToJsonl,
  countProjectSessions,
  deleteSessionJsonl,
  filterSessionRecords,
  formatRelativeAge,
  formatTimestamp,
  formatTokensM,
  loadSessionPreviewMessages,
  type NameFilter,
  type PreviewMessage,
  type SessionRecord,
  type SessionRecordDetails,
  type SessionScope,
  type SessionSearchHit,
  type SortMode,
} from "./session-data.ts";

export { fitWidth, wrapTextToWidth, type TuiTheme as SessionPickerTheme };

export interface SessionPickerTuiOptions {
  records: readonly SessionRecord[];
  currentCwd: string;
  initialQuery?: string;
  initialScope?: SessionScope;
  /** Optional custom title badge (e.g. `(@session)` vs `/resume`). */
  titleBadge?: string;
  /** Optional custom confirm hint label (e.g. `insert` vs `resume`). */
  confirmLabel?: string;
  /**
   * Render as a full-screen edge-to-edge Codex-style split workspace
   * (used by `/resume`) instead of a floating bordered modal box.
   */
  fullscreen?: boolean;
  /** Optional custom preview loader (injected in tests). */
  loadPreview?: (sessionPath: string) => PreviewMessage[];
  /** Optional lazy details loader (`session_details_cache`) for the selected session. */
  loadDetails?: (sessionPath: string) => SessionRecordDetails | null;
  /** Optional full-text database search across all historical sessions. */
  searchRecords?: (options: {
    query: string;
    scope: SessionScope;
    currentCwd: string;
    nameFilter?: NameFilter;
    skipMissingFiles?: boolean;
  }) => SessionSearchHit[];
  /** Debounce delay in ms for `searchRecords` full-text SQLite queries (0 = synchronous). */
  searchDebounceMs?: number;
  /** Path of the active Pi session; it cannot be deleted from the picker. */
  currentSessionPath?: string;
  /** Injected JSONL mutations (defaults to the session-data helpers). */
  deleteSession?: (sessionPath: string) => { ok: boolean; error?: string };
  renameSession?: (sessionPath: string, name: string) => { ok: boolean; error?: string };
  /** Pi-native Markdown renderer for message previews (plain wrapping fallback). */
  markdownRenderer?: (text: string, width: number) => string[];
  /** Terminal height provider for responsive body sizing. */
  getRows?: () => number;
}

type FullscreenLeftRow =
  | { kind: "blank" }
  | { kind: "group"; label: string; count: number }
  | { kind: "item"; hitIndex: number; hit: SessionSearchHit };

function shortenHomePath(cwd: string): string {
  if (!cwd) return "~";
  const home = os.homedir();
  if (home && cwd === home) return "~";
  if (home && cwd.startsWith(`${home}/`)) {
    return `~${cwd.slice(home.length)}`;
  }
  return cwd;
}

/**
 * Return the trailing model identifier after the last `/` (e.g.
 * `magpie/auto/deepseek-v4.1-flash` -> `deepseek-v4.1-flash`).
 */
export function shortModelName(model: string): string {
  const trimmed = model.trim();
  if (!trimmed) return "—";
  const parts = trimmed.split("/").filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1]! : trimmed;
}

/**
 * Curated Codex-style pastel identity palette for dark terminals
 * (high luminance, low eye-strain, matching Codex's syntax-theme accents).
 */
const SESSION_IDENTITY_PALETTE_DARK: readonly string[] = [
  "\x1b[38;2;121;192;255m", // soft sky (#79C0FF)
  "\x1b[38;2;124;232;192m", // soft mint (#7CE8C0)
  "\x1b[38;2;210;168;255m", // soft lavender (#D2A8FF)
  "\x1b[38;2;255;166;87m",  // warm apricot (#FFA657)
  "\x1b[38;2;255;158;190m", // soft rose (#FF9EBE)
  "\x1b[38;2;156;229;138m", // soft sage (#9CE58A)
  "\x1b[38;2;108;212;240m", // soft cyan (#6CD4F0)
  "\x1b[38;2;242;184;181m", // soft coral (#F2B8B5)
];

/** Richer identity palette for light terminal backgrounds. */
const SESSION_IDENTITY_PALETTE_LIGHT: readonly string[] = [
  "\x1b[38;2;9;105;218m",   // github blue (#0969DA)
  "\x1b[38;2;15;118;110m",  // deep teal (#0F766E)
  "\x1b[38;2;130;80;223m",  // soft purple (#8250DF)
  "\x1b[38;2;188;76;0m",    // warm rust (#BC4C00)
  "\x1b[38;2;191;57;137m",  // berry rose (#BF3989)
  "\x1b[38;2;26;127;55m",   // forest green (#1A7F37)
  "\x1b[38;2;2;132;199m",   // cerulean (#0284C7)
  "\x1b[38;2;154;103;0m",   // warm ochre (#9A6700)
];

function isLightTerminalTheme(theme: TuiTheme): boolean {
  const name = String((theme as { name?: unknown }).name ?? "").toLowerCase();
  if (name.includes("light")) return true;
  if (name.includes("dark")) return false;
  const fgbg = process.env.COLORFGBG ?? "";
  const bg = fgbg.split(";").pop();
  if (bg === "15" || bg === "7") return true;
  return false;
}

/**
 * Deterministic FNV-1a identity color per session ID (matches Codex's
 * `thread_color::color_for_id` so colors stay stable across sorts & filters).
 */
function styleSessionTitle(sessionId: string, text: string, lightBg = false): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < sessionId.length; i++) {
    hash ^= sessionId.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  const palette = lightBg ? SESSION_IDENTITY_PALETTE_LIGHT : SESSION_IDENTITY_PALETTE_DARK;
  const color = palette[hash % palette.length]!;
  return `${color}${text}\x1b[39m`;
}

function formatCompactDate(raw: string): string {
  const full = formatTimestamp(raw);
  // `YYYY-MM-DD HH:mm` -> `MM-DD HH:mm`
  return full.length >= 16 ? full.slice(5, 16) : full;
}

export class SessionPickerTui implements Component {
  private records: readonly SessionRecord[];
  private readonly currentCwd: string;
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly titleBadge: string;
  private readonly confirmLabel: string;
  private readonly fullscreen: boolean;
  private readonly done: (selected: SessionRecord | undefined) => void;
  private readonly loadPreview: (sessionPath: string) => PreviewMessage[];
  private readonly loadDetails?: (sessionPath: string) => SessionRecordDetails | null;
  private readonly searchRecords?: (options: {
    query: string;
    scope: SessionScope;
    currentCwd: string;
    nameFilter?: NameFilter;
    skipMissingFiles?: boolean;
  }) => SessionSearchHit[];
  private readonly searchDebounceMs: number;
  private readonly getRows: () => number;
  private readonly currentSessionPath?: string;
  private readonly deleteSession: (sessionPath: string) => { ok: boolean; error?: string };
  private readonly renameSession: (
    sessionPath: string,
    name: string,
  ) => { ok: boolean; error?: string };

  private scope: SessionScope;
  private query: string;
  /** Visual focus indicator when user presses `/` on empty fullscreen search. */
  private searchFocused = false;
  private hits: SessionSearchHit[] = [];
  private searchTimer: ReturnType<typeof setTimeout> | null = null;
  private searchPending = false;
  private sortMode: SortMode = "threaded";
  private nameFilter: NameFilter = "all";
  private showPath = false;
  private allFilteredHits: SessionSearchHit[] = [];
  private mode: "list" | "rename" | "deleteConfirm" = "list";
  private renameDraft = "";
  private deleteTargetPath: string | null = null;
  private statusMessage: { text: string; error?: boolean } | null = null;
  private statusTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly previewRender: RenderCoalescer;
  private readonly markdownRenderer?: (text: string, width: number) => string[];
  /** Wrapped preview lines cached per session + width so wheel scrolling is a slice. */
  private readonly previewLineCache = new Map<string, string[]>();
  private previewLineCacheKey = "";
  /** In-memory rename overrides so SQLite hits outside the initial slice keep their new name. */
  private readonly renamedNames = new Map<string, string>();
  /** Visual row list for fullscreen project-grouped mode. */
  private groupedRows: FullscreenLeftRow[] = [];
  /** O(1) lookup from `hitIndex` to row index in `groupedRows`. */
  private hitIndexToRowIndex: number[] = [];
  private selectedIndex = 0;
  private listScroll = 0;
  private previewScroll = 0;
  private readonly previewCache = new Map<string, PreviewMessage[]>();
  private readonly detailsCache = new Map<string, SessionRecordDetails | null>();
  private lastPreviewLinesCount = 0;

  // Layout geometry captured on each render for accurate mouse hit-testing.
  /** In-memory split ratio only; never persisted. */
  private leftRatio = 0.64;
  private draggingDivider = false;
  private lastTotalWidth = 0;
  private dividerX = 41;
  private bodyStartY = 5;
  private bodyHeight = 14;
  private previewVisibleRows = 10;
  private pillRects = new Map<string, ClickRegion>();
  private closeBtnRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private listRowMap: Array<{ y1: number; y2: number; index: number }> = [];

  private globalCount = 0;
  private projectCount = 0;

  constructor(
    tui: { requestRender: () => void },
    theme: TuiTheme,
    options: SessionPickerTuiOptions,
    done: (selected: SessionRecord | undefined) => void,
  ) {
    this.tui = tui;
    this.theme = theme;
    this.records = options.records;
    this.currentCwd = options.currentCwd;
    this.scope = options.initialScope ?? "global";
    this.query = options.initialQuery ?? "";
    this.titleBadge = options.titleBadge ?? "(@session)";
    this.confirmLabel = options.confirmLabel ?? "insert";
    this.fullscreen = options.fullscreen === true;
    this.done = done;
    this.loadPreview = options.loadPreview ?? ((p) => loadSessionPreviewMessages(p));
    this.loadDetails = options.loadDetails;
    this.searchRecords = options.searchRecords;
    this.searchDebounceMs = Math.max(0, options.searchDebounceMs ?? 0);
    this.getRows = options.getRows ?? (() => 28);
    this.currentSessionPath = options.currentSessionPath;
    this.deleteSession = options.deleteSession ?? ((p) => deleteSessionJsonl(p));
    this.renameSession = options.renameSession ?? ((p, n) => appendSessionNameToJsonl(p, n));
    this.markdownRenderer = options.markdownRenderer;
    this.previewRender = new RenderCoalescer(() => this.tui.requestRender(), 40);

    this.recountSessions();
    this.recomputeHits(true);
  }

  /** Current scope exposed for tests. */
  getScope(): SessionScope {
    return this.scope;
  }

  /** Current query exposed for tests. */
  getQuery(): string {
    return this.query;
  }

  /** Currently selected session record, if any. */
  getSelectedRecord(): SessionRecord | undefined {
    return this.hits[this.selectedIndex]?.record;
  }

  private bold(text: string): string {
    return this.theme.bold ? this.theme.bold(text) : `\x1b[1m${text}\x1b[0m`;
  }

  private fg(color: string, text: string): string {
    return this.theme.fg(color, text);
  }

  private clearSearchTimer(): void {
    if (this.searchTimer !== null) {
      clearTimeout(this.searchTimer);
      this.searchTimer = null;
    }
    this.searchPending = false;
  }

  private clearStatusTimer(): void {
    if (this.statusTimer !== null) {
      clearTimeout(this.statusTimer);
      this.statusTimer = null;
    }
  }

  private setStatus(text: string, error = false): void {
    this.clearStatusTimer();
    this.statusMessage = { text, error };
    this.statusTimer = setTimeout(() => {
      this.statusTimer = null;
      this.statusMessage = null;
      this.tui.requestRender();
    }, 1800);
  }

  private finish(selected: SessionRecord | undefined): void {
    this.clearSearchTimer();
    this.clearStatusTimer();
    this.previewRender.cancel();
    this.draggingDivider = false;
    this.done(selected);
  }

  private applyFilteredHits(filtered: SessionSearchHit[], prevId?: string): void {
    this.allFilteredHits = filtered;
    this.applyCurrentSort(prevId);
  }

  private applyCurrentSort(prevId?: string): void {
    const compareDesc = (a: string, b: string) => (b > a ? 1 : b < a ? -1 : 0);
    let filtered = this.allFilteredHits;
    if (this.sortMode === "recent") {
      filtered = [...filtered].sort((a, b) =>
        compareDesc(a.record.modified, b.record.modified),
      );
    } else if (this.sortMode === "relevance") {
      filtered = [...filtered].sort(
        (a, b) =>
          (b.score ?? 0) - (a.score ?? 0) ||
          compareDesc(a.record.modified, b.record.modified),
      );
    }

    if (this.fullscreen && this.sortMode === "threaded") {
      // Group by project CWD in order of most-recently-updated project so
      // `this.hits` matches the exact visual order rendered in the left pane.
      const groups = new Map<string, SessionSearchHit[]>();
      for (const hit of filtered) {
        const key = hit.record.cwd || hit.record.project || "~";
        const bucket = groups.get(key);
        if (bucket) {
          bucket.push(hit);
        } else {
          groups.set(key, [hit]);
        }
      }
      const ordered: SessionSearchHit[] = [];
      const rows: FullscreenLeftRow[] = [];
      const indexMap: number[] = [];
      let groupIdx = 0;
      for (const [cwdKey, items] of groups) {
        if (groupIdx > 0) {
          rows.push({ kind: "blank" });
        }
        rows.push({
          kind: "group",
          label: shortenHomePath(cwdKey),
          count: items.length,
        });
        for (const item of items) {
          const hitIndex = ordered.length;
          ordered.push(item);
          indexMap.push(rows.length);
          rows.push({ kind: "item", hitIndex, hit: item });
        }
        groupIdx += 1;
      }
      this.hits = ordered;
      this.groupedRows = rows;
      this.hitIndexToRowIndex = indexMap;
    } else if (this.fullscreen) {
      this.hits = filtered;
      this.groupedRows = filtered.map((hit, hitIndex) => ({
        kind: "item",
        hitIndex,
        hit,
      }));
      this.hitIndexToRowIndex = filtered.map((_, hitIndex) => hitIndex);
    } else {
      this.hits = filtered;
      this.groupedRows = [];
      this.hitIndexToRowIndex = [];
    }
    if (prevId) {
      const keptIdx = this.hits.findIndex((h) => h.record.id === prevId);
      this.selectedIndex = keptIdx >= 0 ? keptIdx : 0;
    } else {
      this.selectedIndex = 0;
    }
    this.listScroll = 0;
    this.previewScroll = 0;
    this.clampListScroll();
  }

  private recomputeHits(immediate = false): void {
    this.clearSearchTimer();
    if (!immediate && this.searchDebounceMs > 0 && this.query.trim()) {
      this.searchPending = true;
      this.searchTimer = setTimeout(() => {
        this.recomputeHits(true);
        this.tui.requestRender();
      }, this.searchDebounceMs);
      return;
    }

    const prevId = this.hits[this.selectedIndex]?.record.id;
    let filtered = filterSessionRecords(this.records, {
      scope: this.scope,
      currentCwd: this.currentCwd,
      query: this.query,
      nameFilter: this.nameFilter,
    });
    if (this.query.trim() && this.searchRecords) {
      const dbHits = this.searchRecords({
        query: this.query,
        scope: this.scope,
        currentCwd: this.currentCwd,
        nameFilter: this.nameFilter,
        skipMissingFiles: true,
      });
      if (dbHits.length > 0) {
        const seen = new Set(filtered.map((h) => h.record.id));
        const merged = [...filtered];
        for (const hit of dbHits) {
          const renamed = this.renamedNames.get(hit.record.path);
          const record = renamed ? { ...hit.record, name: renamed, title: renamed } : hit.record;
          if (this.nameFilter === "named" && !record.name?.trim()) continue;
          if (!seen.has(hit.record.id)) {
            seen.add(hit.record.id);
            merged.push(renamed ? { ...hit, record } : hit);
          }
        }
        filtered = merged;
      }
    }
    this.applyFilteredHits(filtered, prevId);
  }

  private setScope(next: SessionScope): void {
    if (this.scope === next) return;
    this.scope = next;
    this.recomputeHits();
  }

  private toggleScope(): void {
    this.setScope(this.scope === "global" ? "project" : "global");
  }

  private recountSessions(): void {
    this.globalCount = this.records.length;
    this.projectCount = countProjectSessions(this.records, this.currentCwd);
  }

  private cycleSortMode(): void {
    this.sortMode =
      this.sortMode === "threaded"
        ? "recent"
        : this.sortMode === "recent"
          ? "relevance"
          : "threaded";
    this.applyCurrentSort(this.hits[this.selectedIndex]?.record.id);
    const label =
      this.sortMode === "threaded"
        ? "Threaded"
        : this.sortMode === "recent"
          ? "Recent"
          : "Relevance";
    this.setStatus(`Sort: ${label}`);
    this.tui.requestRender();
  }

  private toggleNameFilter(): void {
    this.nameFilter = this.nameFilter === "all" ? "named" : "all";
    this.setStatus(`Name: ${this.nameFilter === "named" ? "Named" : "All"}`);
    this.recomputeHits(true);
    this.tui.requestRender();
  }

  private togglePath(): void {
    this.showPath = !this.showPath;
    this.setStatus(`Path ${this.showPath ? "on" : "off"}`);
    this.tui.requestRender();
  }

  private isCurrentSession(record: SessionRecord): boolean {
    if (!this.currentSessionPath) return false;
    return path.resolve(record.path) === path.resolve(this.currentSessionPath);
  }

  private requestDelete(): void {
    const selected = this.getSelectedRecord();
    if (!selected) return;
    if (this.isCurrentSession(selected)) {
      this.setStatus("Cannot delete the currently active session", true);
      this.tui.requestRender();
      return;
    }
    this.deleteTargetPath = selected.path;
    this.mode = "deleteConfirm";
    this.tui.requestRender();
  }

  private confirmDelete(): void {
    const target = this.deleteTargetPath;
    this.deleteTargetPath = null;
    this.mode = "list";
    if (!target) return;
    const result = this.deleteSession(target);
    if (!result.ok) {
      this.setStatus(`Failed to delete: ${result.error ?? "Unknown error"}`, true);
      this.tui.requestRender();
      return;
    }
    this.records = this.records.filter((record) => record.path !== target);
    this.previewCache.delete(target);
    this.detailsCache.delete(target);
    for (const key of [...this.previewLineCache.keys()]) {
      if (key.startsWith(`${target}:`)) this.previewLineCache.delete(key);
    }
    this.recountSessions();
    this.recomputeHits(true);
    this.setStatus("Session deleted");
    this.tui.requestRender();
  }

  private startRename(): void {
    const selected = this.getSelectedRecord();
    if (!selected) return;
    this.renameDraft = selected.name ?? "";
    this.mode = "rename";
    this.tui.requestRender();
  }

  private confirmRename(): void {
    const target = this.getSelectedRecord();
    const next = this.renameDraft.replace(/[\r\n]+/g, " ").trim();
    this.renameDraft = "";
    this.mode = "list";
    if (!target || !next) return;
    const result = this.renameSession(target.path, next);
    if (!result.ok) {
      this.setStatus(`Failed to rename: ${result.error ?? "Unknown error"}`, true);
      this.tui.requestRender();
      return;
    }
    this.records = this.records.map((record) =>
      record.path === target.path ? { ...record, name: next, title: next } : record,
    );
    this.renamedNames.set(target.path, next);
    this.recomputeHits(true);
    this.setStatus("Session renamed");
    this.tui.requestRender();
  }

  private itemsPerPage(): number {
    if (this.fullscreen) {
      // Fullscreen mode reserves 1 row inside body for the column header.
      return Math.max(1, this.bodyHeight - 1);
    }
    // Each session card occupies 2 lines (title + compact metadata) in the left pane.
    return Math.max(1, Math.floor(this.bodyHeight / 2));
  }

  private clampListScroll(): void {
    const page = this.itemsPerPage();
    if (this.hits.length === 0) {
      this.selectedIndex = 0;
      this.listScroll = 0;
      return;
    }
    this.selectedIndex = Math.max(0, Math.min(this.hits.length - 1, this.selectedIndex));
    if (this.fullscreen) {
      const selectedRowIdx = this.hitIndexToRowIndex[this.selectedIndex] ?? -1;
      const targetRow = selectedRowIdx >= 0 ? selectedRowIdx : 0;
      // Keep the group header visible above the first item of a group when scrolling up.
      const headerRow =
        targetRow > 0 && this.groupedRows[targetRow - 1]?.kind === "group"
          ? targetRow - 1
          : targetRow;
      if (headerRow < this.listScroll) {
        this.listScroll = headerRow;
      } else if (targetRow >= this.listScroll + page) {
        this.listScroll = targetRow - page + 1;
      }
      const maxScroll = Math.max(0, this.groupedRows.length - page);
      this.listScroll = Math.max(0, Math.min(maxScroll, this.listScroll));
      return;
    }
    if (this.selectedIndex < this.listScroll) {
      this.listScroll = this.selectedIndex;
    } else if (this.selectedIndex >= this.listScroll + page) {
      this.listScroll = this.selectedIndex - page + 1;
    }
    const maxScroll = Math.max(0, this.hits.length - page);
    this.listScroll = Math.max(0, Math.min(maxScroll, this.listScroll));
  }

  private moveSelection(delta: number): void {
    if (this.hits.length === 0) return;
    const prev = this.selectedIndex;
    this.selectedIndex = Math.max(0, Math.min(this.hits.length - 1, this.selectedIndex + delta));
    if (this.selectedIndex !== prev) {
      this.previewScroll = 0;
    }
    this.clampListScroll();
  }

  private scrollPreview(delta: number): void {
    const maxScroll = Math.max(0, this.lastPreviewLinesCount - this.previewVisibleRows);
    this.previewScroll = Math.max(0, Math.min(maxScroll, this.previewScroll + delta));
  }

  private getPreviewMessages(record: SessionRecord): PreviewMessage[] {
    const cached = this.previewCache.get(record.path);
    if (cached) return cached;
    const loaded = this.loadPreview(record.path);
    this.previewCache.set(record.path, loaded);
    return loaded;
  }

  private hydrateDetails(record: SessionRecord): SessionRecord {
    if (!this.loadDetails || record.userMessages !== null) return record;
    let details = this.detailsCache.get(record.path);
    if (details === undefined) {
      details = this.loadDetails(record.path);
      this.detailsCache.set(record.path, details);
    }
    if (!details) return record;
    return {
      ...record,
      userMessages: details.userMessages,
      assistantMessages: details.assistantMessages,
      totalTokens: details.totalTokens,
      model:
        record.model && record.model !== "—"
          ? record.model
          : details.model || record.model,
    };
  }

  private renderMessageContent(text: string, width: number): string[] {
    return this.markdownRenderer?.(text, width) ?? wrapTextToWidth(text, width);
  }

  /** Required by Pi TUI Component interface. */
  invalidate(): void {
    // Re-renders directly from live state on every frame.
  }

  handleInput(data: string): void {
    if (this.mode === "deleteConfirm") {
      if (matchesKey(data, "enter") || matchesKey(data, "return")) {
        this.confirmDelete();
      } else if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
        this.mode = "list";
        this.deleteTargetPath = null;
        this.tui.requestRender();
      }
      return;
    }

    if (this.mode === "rename") {
      if (matchesKey(data, "enter") || matchesKey(data, "return")) {
        this.confirmRename();
        return;
      }
      if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
        this.mode = "list";
        this.renameDraft = "";
        this.tui.requestRender();
        return;
      }
      if (matchesKey(data, "backspace")) {
        this.renameDraft = Array.from(this.renameDraft).slice(0, -1).join("");
        this.tui.requestRender();
        return;
      }
      let renameInput = data;
      if (renameInput.startsWith("\x1b[200~") && renameInput.endsWith("\x1b[201~")) {
        renameInput = renameInput.slice(6, -6).replace(/[\r\n]+/g, " ");
      }
      if (!renameInput.startsWith("\x1b") && !/[\x00-\x1f\x7f]/.test(renameInput)) {
        this.renameDraft += renameInput;
        this.tui.requestRender();
      }
      return;
    }

    if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
      this.finish(undefined);
      return;
    }
    if (matchesKey(data, "ctrl+s")) {
      this.cycleSortMode();
      return;
    }
    if (matchesKey(data, "ctrl+n")) {
      this.toggleNameFilter();
      return;
    }
    if (matchesKey(data, "ctrl+d")) {
      this.requestDelete();
      return;
    }
    if (matchesKey(data, "ctrl+p")) {
      this.togglePath();
      return;
    }
    if (matchesKey(data, "ctrl+r")) {
      this.startRename();
      return;
    }
    if (matchesKey(data, "enter") || matchesKey(data, "return")) {
      if (this.searchPending) this.recomputeHits(true);
      this.finish(this.getSelectedRecord());
      return;
    }
    if (matchesKey(data, "tab") || matchesKey(data, "shift+tab") || matchesKey(data, "ctrl+g")) {
      this.toggleScope();
      return;
    }
    if (matchesKey(data, "up")) {
      this.moveSelection(-1);
      return;
    }
    if (matchesKey(data, "down")) {
      this.moveSelection(1);
      return;
    }
    if (matchesKey(data, "pageUp") || matchesKey(data, "ctrl+u")) {
      this.scrollPreview(-Math.max(3, Math.floor(this.previewVisibleRows / 2)));
      return;
    }
    if (matchesKey(data, "pageDown")) {
      this.scrollPreview(Math.max(3, Math.floor(this.previewVisibleRows / 2)));
      return;
    }
    if (matchesKey(data, "ctrl+w") || matchesKey(data, "ctrl+l")) {
      if (this.query.length > 0 || this.searchFocused) {
        this.query = "";
        this.searchFocused = false;
        this.recomputeHits();
      }
      return;
    }
    if (matchesKey(data, "backspace")) {
      if (this.query.length > 0) {
        this.query = Array.from(this.query).slice(0, -1).join("");
        this.recomputeHits();
      } else if (this.searchFocused) {
        this.searchFocused = false;
      }
      return;
    }

    // Handle bracketed paste (`\x1b[200~...\x1b[201~`) or printable characters.
    let printable = data;
    if (printable.startsWith("\x1b[200~") && printable.endsWith("\x1b[201~")) {
      printable = printable.slice(6, -6).replace(/[\r\n]+/g, " ");
    }
    if (!printable.startsWith("\x1b") && !/[\x00-\x1f\x7f]/.test(printable)) {
      // Pressing `/` on an empty fullscreen search bar focuses search cleanly
      // instead of inserting a duplicate literal `/` next to the `/` prefix.
      if (this.fullscreen && this.query.length === 0 && !this.searchFocused && printable === "/") {
        this.searchFocused = true;
        return;
      }
      this.query += printable;
      this.recomputeHits();
    }
  }

  private setDividerFromX(x: number): void {
    const minRight = this.lastTotalWidth >= 80 ? 28 : 20;
    const maxLeft = this.lastTotalWidth - minRight - 1;
    const left = Math.max(20, Math.min(x, maxLeft));
    this.leftRatio = left / Math.max(1, this.lastTotalWidth - 1);
  }

  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    const { x, y, type } = event;
    if (this.mode !== "list") return { handled: true, render: false };

    if (this.fullscreen && this.draggingDivider) {
      if (type === "drag") {
        this.setDividerFromX(x);
        return { handled: true, capture: true, render: true };
      }
      if (type === "release") {
        this.draggingDivider = false;
        return { handled: true, render: true };
      }
    }

    if (this.fullscreen && type === "press" && event.button === "left") {
      const inBody = y >= this.bodyStartY && y < this.bodyStartY + this.bodyHeight;
      if (inBody && Math.abs(x - this.dividerX) <= 1) {
        this.draggingDivider = true;
        return { handled: true, capture: true, render: true };
      }
    }

    if (type === "wheel") {
      const delta = (event.wheelDelta ?? 0) > 0 ? 1 : -1;
      if (y >= this.bodyStartY && y < this.bodyStartY + this.bodyHeight) {
        if (x < this.dividerX) {
          this.moveSelection(delta);
          return { handled: true, render: true };
        }
        this.scrollPreview(delta * 3);
        this.previewRender.schedule();
        return { handled: true, render: false };
      }
      return { handled: true, render: false };
    }

    if (type === "press" && event.button === "left") {
      // Header bar clicks (scope pills & close button).
      if (hitRegion(this.closeBtnRect, x, y)) {
        this.finish(undefined);
        return { handled: true, render: false };
      }
      if (hitRegion(this.pillRects.get("global"), x, y)) {
        this.setScope("global");
        return { handled: true, render: true };
      }
      if (hitRegion(this.pillRects.get("project"), x, y)) {
        this.setScope("project");
        return { handled: true, render: true };
      }
      // Left session list row selection.
      const leftMinX = this.fullscreen ? 0 : 1;
      if (x >= leftMinX && x < this.dividerX) {
        for (const row of this.listRowMap) {
          if (y >= row.y1 && y <= row.y2) {
            if (this.selectedIndex !== row.index) {
              this.selectedIndex = row.index;
              this.previewScroll = 0;
              this.clampListScroll();
            }
            return { handled: true, render: true };
          }
        }
      }
      return { handled: true, render: false };
    }

    if (type === "click" && event.button === "left" && (event.clickCount ?? 1) >= 2) {
      const leftMinX = this.fullscreen ? 0 : 1;
      if (x >= leftMinX && x < this.dividerX) {
        for (const row of this.listRowMap) {
          if (y >= row.y1 && y <= row.y2) {
            this.selectedIndex = row.index;
            const clicked = this.hits[row.index]?.record;
            if (clicked) this.finish(clicked);
            return { handled: true, render: false };
          }
        }
      }
    }

    return { handled: true, render: false };
  }

  private renderRightPane(innerW: number, height: number): string[] {
    const rawSelected = this.hits[this.selectedIndex]?.record;
    const selected = rawSelected ? this.hydrateDetails(rawSelected) : undefined;
    if (!selected) {
      const empty = [
        "",
        this.fg("dim", "  No session selected."),
        this.fg("dim", "  Try clearing search or switching Global / Project."),
      ];
      while (empty.length < height) empty.push("");
      return empty.slice(0, height).map((l) => fitWidth(l, innerW));
    }

    const pad = (s: string) => ` ${s}`;
    const msgsDetail =
      selected.userMessages !== null && selected.assistantMessages !== null
        ? `${selected.messageCount} (${selected.userMessages}U/${selected.assistantMessages}A)`
        : `${selected.messageCount}`;

    const metaLines: string[] = [
      pad(this.bold(this.fg("accent", selected.title))),
    ];
    if (selected.name?.trim()) {
      metaLines.push(pad(`${this.fg("dim", "Name:")} ${this.fg("warning", selected.name)}`));
    }
    metaLines.push(
      pad(
        `${this.fg("dim", "Project:")} ${this.bold(selected.project)}  ${this.fg("dim", "ID:")} ${selected.id}`,
      ),
      pad(
        `${this.fg("dim", "Created:")} ${formatTimestamp(selected.created)}  ${this.fg("dim", "Updated:")} ${formatTimestamp(selected.modified)}`,
      ),
      pad(
        `${this.fg("dim", "Msgs:")} ${msgsDetail}  ${this.fg("dim", "Tokens:")} ${this.fg("warning", formatTokensM(selected.totalTokens))}  ${this.fg("dim", "Model:")} ${this.fg("success", shortModelName(selected.model))}`,
      ),
    );
    if (this.showPath) {
      for (const pLine of wrapTextToWidth(shortenHomePath(selected.path), Math.max(8, innerW - 2)).slice(0, 2)) {
        metaLines.push(pad(this.fg("dim", pLine)));
      }
    }

    const divider = this.fg("border", "─".repeat(innerW));
    const headerBlock = [...metaLines, divider];
    const previewHeight = Math.max(1, height - headerBlock.length);
    this.previewVisibleRows = previewHeight;

    const textWidth = Math.max(8, innerW - 2);
    const cacheKey = `${selected.path}:bordered:${textWidth}:${this.markdownRenderer ? "md" : "plain"}`;
    if (cacheKey !== this.previewLineCacheKey) {
      this.previewLineCache.clear();
      this.previewLineCacheKey = cacheKey;
    }
    let allPreviewLines = this.previewLineCache.get(cacheKey);
    if (!allPreviewLines) {
      const messages = this.getPreviewMessages(selected);
      allPreviewLines = [];
      if (messages.length === 0) {
        allPreviewLines.push(this.fg("dim", " (No user/agent messages available for preview)"));
      } else {
        for (const msg of messages) {
          const badge =
            msg.role === "user"
              ? this.bold(this.fg("accent", "You"))
              : this.bold(this.fg("success", "Agent"));
          const tsLabel = msg.timestamp ? this.fg("dim", ` ${formatTimestamp(msg.timestamp).slice(11)}`) : "";
          allPreviewLines.push(` ${badge}${tsLabel}`);
          for (const wline of this.renderMessageContent(msg.content, textWidth)) {
            allPreviewLines.push(`  ${wline}`);
          }
          allPreviewLines.push("");
        }
      }
      this.previewLineCache.set(cacheKey, allPreviewLines);
    }

    this.lastPreviewLinesCount = allPreviewLines.length;
    const maxScroll = Math.max(0, allPreviewLines.length - previewHeight);
    if (this.previewScroll > maxScroll) this.previewScroll = maxScroll;

    const visiblePreview = allPreviewLines
      .slice(this.previewScroll, this.previewScroll + previewHeight)
      .map((line) =>
        this.query.trim() ? highlightTerms(line, this.query, (m) => `\x1b[7m${m}\x1b[27m`) : line,
      );
    while (visiblePreview.length < previewHeight) visiblePreview.push("");

    return [...headerBlock, ...visiblePreview]
      .slice(0, height)
      .map((line) => fitWidth(line, innerW));
  }

  private renderLeftPane(innerW: number, height: number): string[] {
    this.listRowMap = [];
    this.clampListScroll();

    if (this.hits.length === 0) {
      const lines = [
        "",
        this.fg("dim", " No matching sessions"),
        this.fg("dim", ` Scope: ${this.scope === "global" ? "Global" : "Project"}`),
      ];
      while (lines.length < height) lines.push("");
      return lines.slice(0, height).map((l) => fitWidth(l, innerW));
    }

    const lines: string[] = [];
    const page = this.itemsPerPage();
    for (let slot = 0; slot < page; slot++) {
      const idx = this.listScroll + slot;
      if (idx >= this.hits.length) break;
      const hit = this.hits[idx]!;
      const rec = hit.record;
      const isSelected = idx === this.selectedIndex;
      const startY = this.bodyStartY + lines.length;

      const marker = isSelected ? this.fg("accent", "▸ ") : "  ";
      const age = formatRelativeAge(rec.modified);
      const rightAge = age ? ` ${age}` : "";
      const titleBudget = Math.max(6, innerW - 2 - visibleWidth(rightAge));
      const titleText = truncateToWidth(rec.title, titleBudget, "");
      const titleStyled = isSelected ? this.bold(this.fg("accent", titleText)) : titleText;
      const gap = Math.max(0, innerW - 2 - visibleWidth(titleText) - visibleWidth(rightAge));
      const row1 = `${marker}${titleStyled}${" ".repeat(gap)}${this.fg("dim", rightAge)}`;

      const pathPrefix = this.showPath ? `${shortenHomePath(rec.path)} · ` : "";
      const metaSummary = hit.matchSnippet
        ? `${pathPrefix}${rec.project} · ${hit.matchSnippet}`
        : `${pathPrefix}${rec.project} · ${rec.shortId} · ${rec.messageCount}m · ${formatTokensM(rec.totalTokens)} · ${shortModelName(rec.model)}`;
      const row2 = `  ${this.fg("dim", truncateToWidth(metaSummary, Math.max(4, innerW - 2), ""))}`;

      lines.push(fitWidth(row1, innerW));
      if (lines.length < height) {
        lines.push(fitWidth(row2, innerW));
      }
      this.listRowMap.push({
        y1: startY,
        y2: this.bodyStartY + lines.length - 1,
        index: idx,
      });
    }

    while (lines.length < height) {
      lines.push(" ".repeat(innerW));
    }
    return lines.slice(0, height);
  }

  private renderFullscreenLeftPane(innerW: number, height: number): string[] {
    this.listRowMap = [];
    this.clampListScroll();

    const ageColW = 7;
    const createdColW = innerW >= 88 ? 11 : 0;
    const msgsColW = innerW >= 68 ? 5 : 0;
    const modelColW = innerW >= 72 ? 18 : 0;
    const rightColsW =
      ageColW +
      (createdColW > 0 ? createdColW + 2 : 0) +
      (msgsColW > 0 ? msgsColW + 2 : 0) +
      (modelColW > 0 ? modelColW + 2 : 0) +
      2;
    const titleColW = Math.max(12, innerW - rightColsW);

    // Column header row (Codex Agent Command Center style).
    const headerLeft = `  ${this.fg("dim", "Sessions")}`;
    const headerMsgs = msgsColW > 0 ? `${fitWidth(this.fg("dim", "Msgs"), msgsColW)}  ` : "";
    const headerModel = modelColW > 0 ? `${fitWidth(this.fg("dim", "Model"), modelColW)}  ` : "";
    const headerCreated =
      createdColW > 0 ? `${fitWidth(this.fg("dim", "Created"), createdColW)}  ` : "";
    const headerAge = fitWidth(this.fg("dim", "Updated"), ageColW);
    const headerRight = `${headerMsgs}${headerModel}${headerCreated}${headerAge}  `;
    const headerGap = Math.max(
      1,
      innerW - visibleWidth(headerLeft) - visibleWidth(headerRight),
    );
    const lines: string[] = [
      fitWidth(`${headerLeft}${" ".repeat(headerGap)}${headerRight}`, innerW),
    ];

    const listHeight = Math.max(1, height - 1);
    if (this.hits.length === 0) {
      lines.push("");
      lines.push(fitWidth(`  ${this.fg("dim", "No matching sessions")}`, innerW));
      lines.push(
        fitWidth(
          `  ${this.fg("dim", `Scope: ${this.scope === "global" ? "All" : "Project"} · press Tab to switch or Backspace to clear search`)}`,
          innerW,
        ),
      );
      while (lines.length < height) lines.push(" ".repeat(innerW));
      return lines.slice(0, height);
    }

    const visibleSlice = this.groupedRows.slice(this.listScroll, this.listScroll + listHeight);
    const lightBg = isLightTerminalTheme(this.theme);
    for (const row of visibleSlice) {
      if (lines.length >= height) break;
      if (row.kind === "blank") {
        lines.push(" ".repeat(innerW));
        continue;
      }
      if (row.kind === "group") {
        const groupLine = `  ${this.fg("dim", row.label)}  ${this.fg("dim", String(row.count))}`;
        lines.push(fitWidth(groupLine, innerW));
        continue;
      }

      const { hitIndex, hit } = row;
      const rec = hit.record;
      const isSelected = hitIndex === this.selectedIndex;
      const rowY = this.bodyStartY + lines.length;

      const ageText = formatRelativeAge(rec.modified) || "—";
      const createdText = createdColW > 0 ? formatCompactDate(rec.created) : "";
      const msgsText = msgsColW > 0 ? String(rec.messageCount) : "";
      const modelText =
        modelColW > 0 ? truncateToWidth(shortModelName(rec.model), modelColW, "") : "";

      const titleGap = 2;
      if (isSelected) {
        const prefix = " ▸ ○ ";
        const titleBudget = Math.max(8, titleColW - visibleWidth(prefix) - titleGap);
        const titleText = truncateToWidth(rec.title, titleBudget, "");
        const leftPart = `${prefix}${titleText}`;
        const rightPart = [
          msgsColW > 0 ? `${fitWidth(msgsText, msgsColW)}  ` : "",
          modelColW > 0 ? `${fitWidth(modelText, modelColW)}  ` : "",
          createdColW > 0 ? `${fitWidth(createdText, createdColW)}  ` : "",
          `${fitWidth(ageText, ageColW)}  `,
        ].join("");
        const gap = Math.max(1, innerW - visibleWidth(leftPart) - visibleWidth(rightPart));
        const plainFull = fitWidth(`${leftPart}${" ".repeat(gap)}${rightPart}`, innerW);
        lines.push(`\x1b[7m${this.bold(plainFull)}\x1b[27m`);
      } else {
        const prefix = "   ○ ";
        const titleBudget = Math.max(8, titleColW - visibleWidth(prefix) - titleGap);
        const titleRaw = truncateToWidth(rec.title, titleBudget, "");
        const coloredTitle = styleSessionTitle(
          rec.id,
          titleRaw,
          lightBg,
        );
        const titleStyled = this.query.trim()
          ? highlightTerms(coloredTitle, this.query, (m) => `\x1b[7m${m}\x1b[27m`)
          : coloredTitle;
        const leftPart = `${this.fg("dim", prefix)}${titleStyled}`;
        const rightPart = [
          msgsColW > 0 ? `${fitWidth(this.fg("dim", msgsText), msgsColW)}  ` : "",
          modelColW > 0 ? `${fitWidth(this.fg("dim", modelText), modelColW)}  ` : "",
          createdColW > 0 ? `${fitWidth(this.fg("dim", createdText), createdColW)}  ` : "",
          `${fitWidth(this.fg("dim", ageText), ageColW)}  `,
        ].join("");
        const gap = Math.max(1, innerW - visibleWidth(leftPart) - visibleWidth(rightPart));
        lines.push(fitWidth(`${leftPart}${" ".repeat(gap)}${rightPart}`, innerW));
      }

      this.listRowMap.push({ y1: rowY, y2: rowY, index: hitIndex });
    }

    while (lines.length < height) {
      lines.push(" ".repeat(innerW));
    }
    return lines.slice(0, height);
  }

  private renderFullscreenRightPane(innerW: number, height: number): string[] {
    const pad = (s: string) => `  ${s}`;
    const textW = Math.max(10, innerW - 3);
    const rawSelected = this.hits[this.selectedIndex]?.record;
    const selected = rawSelected ? this.hydrateDetails(rawSelected) : undefined;
    const selectedHit = this.hits[this.selectedIndex];
    if (!selected) {
      const empty = [
        pad(this.fg("dim", "Session details")),
        "",
        pad(this.fg("dim", "No session selected.")),
        pad(this.fg("dim", "Press Tab to switch All / Project or clear search.")),
      ];
      while (empty.length < height) empty.push("");
      return empty.slice(0, height).map((l) => fitWidth(l, innerW));
    }

    const msgsDetail =
      selected.userMessages !== null && selected.assistantMessages !== null
        ? `${selected.messageCount} (${selected.userMessages}U/${selected.assistantMessages}A)`
        : `${selected.messageCount}`;

    const detailLabel = this.fg("dim", "Session details");
    const titleBudget = Math.max(8, textW - 2 - visibleWidth("Session details") - 1);
    const wrappedTitle = wrapTextToWidth(selected.title, titleBudget).slice(0, 2);
    const headerLines: string[] = [];
    if (wrappedTitle.length === 0) {
      headerLines.push(pad(detailLabel));
    } else {
      wrappedTitle.forEach((tLine, idx) => {
        const styled = this.bold(
          styleSessionTitle(selected.id, tLine, isLightTerminalTheme(this.theme)),
        );
        headerLines.push(pad(idx === 0 ? `${detailLabel} ${styled}` : styled));
      });
    }

    const field = (label: string, value: string, color = "dim") => {
      const budget = Math.max(8, textW - 2 - visibleWidth(label) - 1);
      return pad(
        `${this.fg("dim", label)} ${this.fg(color, truncateToWidth(value, budget, ""))}`,
      );
    };

    headerLines.push(
      pad(
        `${this.fg("dim", "○")} ${this.fg("dim", selected.shortId)} · ${this.fg("dim", "Messages:")} ${msgsDetail}`,
      ),
    );
    headerLines.push(
      field("Model:", `${shortModelName(selected.model)} · ${formatTokensM(selected.totalTokens)}`, "success"),
    );
    if (selected.name?.trim()) headerLines.push(field("Name:", selected.name, "warning"));
    headerLines.push(field("Created:", formatCompactDate(selected.created)));
    headerLines.push(field("Updated:", formatCompactDate(selected.modified)));
    headerLines.push(field("Project:", selected.cwd || selected.project));
    if (this.showPath) headerLines.push(field("Path:", shortenHomePath(selected.path)));
    if (selectedHit?.matchSnippet && this.query.trim()) {
      headerLines.push(pad(this.fg("dim", "Match")));
      headerLines.push(
        pad(
          highlightTerms(
            truncateToWidth(selectedHit.matchSnippet, textW, ""),
            this.query,
            (m) => `\x1b[7m${m}\x1b[27m`,
          ),
        ),
      );
    }
    headerLines.push(pad(this.fg("dim", "Last messages")));

    const previewHeight = Math.max(1, height - headerLines.length);
    this.previewVisibleRows = previewHeight;

    const cacheKey = `${selected.path}:${textW}:${this.markdownRenderer ? "md" : "plain"}`;
    if (cacheKey !== this.previewLineCacheKey) {
      this.previewLineCache.clear();
      this.previewLineCacheKey = cacheKey;
    }
    let allPreviewLines = this.previewLineCache.get(cacheKey);
    if (!allPreviewLines) {
      const messages = this.getPreviewMessages(selected);
      allPreviewLines = [];
      if (messages.length === 0) {
        allPreviewLines.push(pad(this.fg("dim", "(No user/agent messages available)")));
      } else {
        for (const msg of messages) {
          const badge =
            msg.role === "user"
              ? this.bold(this.fg("accent", "You"))
              : this.bold(this.fg("success", "Agent"));
          const tsLabel = msg.timestamp
            ? this.fg("dim", ` ${formatTimestamp(msg.timestamp).slice(11)}`)
            : "";
          allPreviewLines.push(pad(`${badge}${tsLabel}`));
          for (const wline of this.renderMessageContent(msg.content, textW)) {
            allPreviewLines.push(pad(wline));
          }
          allPreviewLines.push("");
        }
      }
      this.previewLineCache.set(cacheKey, allPreviewLines);
    }

    this.lastPreviewLinesCount = allPreviewLines.length;
    const maxScroll = Math.max(0, allPreviewLines.length - previewHeight);
    if (this.previewScroll > maxScroll) this.previewScroll = maxScroll;

    const visiblePreview = allPreviewLines
      .slice(this.previewScroll, this.previewScroll + previewHeight)
      .map((line) =>
        this.query.trim() ? highlightTerms(line, this.query, (m) => `\x1b[7m${m}\x1b[27m`) : line,
      );
    while (visiblePreview.length < previewHeight) visiblePreview.push("");

    return [...headerLines, ...visiblePreview]
      .slice(0, height)
      .map((line) => fitWidth(line, innerW));
  }

  private renderActionPanel(width: number, height: number): string[] {
    const lines: string[] = Array.from({ length: height }, () => " ".repeat(width));
    const start = Math.max(0, Math.floor((height - 4) / 2));
    const title =
      this.mode === "rename" ? "Rename Session" : "Delete session?";
    const detail =
      this.mode === "rename"
        ? `  > ${this.renameDraft}▏`
        : `  ${shortenHomePath(this.deleteTargetPath ?? "")}`;
    const setLine = (row: number, text: string) => {
      if (row >= 0 && row < height) lines[row] = fitWidth(text, width);
    };
    setLine(start, `  ${this.bold(title)}`);
    setLine(start + 1, detail);
    setLine(start + 2, `  ${this.fg("dim", "Enter confirm · Esc cancel")}`);
    return lines;
  }

  private renderFullscreen(width: number): string[] {
    const totalWidth = Math.max(40, width);
    const totalRows = Math.max(12, this.getRows());
    // Reserve 3 top rows, 1 shortcut row and 1 bottom footer row.
    const bodyHeight = Math.max(4, totalRows - 5);
    const minRight = totalWidth >= 80 ? 28 : 20;
    this.lastTotalWidth = totalWidth;
    const leftWidth = Math.max(
      20,
      Math.min(totalWidth - minRight - 1, Math.floor((totalWidth - 1) * this.leftRatio)),
    );
    const rightWidth = Math.max(10, totalWidth - 1 - leftWidth);

    this.dividerX = leftWidth;
    this.bodyStartY = 3;
    this.bodyHeight = bodyHeight;

    // Row 0: Codex-style top title bar + clickable close button.
    const sortLabel =
      this.sortMode === "threaded"
        ? "Group: Project"
        : this.sortMode === "recent"
          ? "Sort: Recent"
          : "Sort: Relevance";
    const titleLeft = `  ${this.bold("Sessions")}  ${this.fg("dim", sortLabel)}`;
    const closeLabel = "[✕]";
    const closeStyled = this.fg("dim", closeLabel);
    const row0Gap = Math.max(1, totalWidth - visibleWidth(titleLeft) - visibleWidth(closeLabel) - 2);
    const closeX1 = totalWidth - visibleWidth(closeLabel) - 2;
    this.closeBtnRect = { x1: closeX1, x2: closeX1 + visibleWidth(closeLabel) - 1, y: 0 };
    const row0 = fitWidth(`${titleLeft}${" ".repeat(row0Gap)}${closeStyled}  `, totalWidth);

    // Row 1: Scope filter tabs (`All N` / `Project M`) + inline search + right key hint.
    this.pillRects = new Map();
    const allRaw = `All ${this.globalCount}`;
    const projRaw = `Project ${this.projectCount}`;
    let cursorX = 2;
    this.pillRects.set("global", {
      x1: cursorX,
      x2: cursorX + visibleWidth(allRaw) - 1,
      y: 1,
    });
    cursorX += visibleWidth(allRaw) + 3;
    this.pillRects.set("project", {
      x1: cursorX,
      x2: cursorX + visibleWidth(projRaw) - 1,
      y: 1,
    });

    const allTab =
      this.scope === "global"
        ? `\x1b[4m${this.bold(this.fg("accent", allRaw))}\x1b[24m`
        : this.fg("dim", allRaw);
    const projTab =
      this.scope === "project"
        ? `\x1b[4m${this.bold(this.fg("accent", projRaw))}\x1b[24m`
        : this.fg("dim", projRaw);
    const searchBlock = this.query || this.searchFocused
      ? `   ${this.fg("accent", "/")} ${this.query}${this.fg("accent", "▏")}${this.searchPending ? this.fg("dim", " searching") : ""}`
      : `   ${this.fg("dim", "/ type to filter...")}`;
    const row1Left = `  ${allTab}   ${projTab}${searchBlock}`;
    const row1Right = this.statusMessage?.error
      ? this.fg("error", this.statusMessage.text)
      : this.fg("dim", this.statusMessage?.text ?? "tab/⇧tab filter  esc close  ");
    const row1Gap = Math.max(1, totalWidth - visibleWidth(row1Left) - visibleWidth(row1Right));
    const row1 = fitWidth(`${row1Left}${" ".repeat(row1Gap)}${row1Right}`, totalWidth);

    // Row 2: Full-width horizontal hairline divider.
    const row2 = this.fg("border", "─".repeat(totalWidth));

    // Body rows: left grouped list | vertical divider | right session details.
    const bodyLines: string[] = [];
    if (this.mode === "list") {
      const leftLines = this.renderFullscreenLeftPane(leftWidth, bodyHeight);
      const rightLines = this.renderFullscreenRightPane(rightWidth, bodyHeight);
      const vBar = this.draggingDivider ? this.fg("accent", "│") : this.fg("border", "│");
      for (let i = 0; i < bodyHeight; i++) {
        bodyLines.push(
          `${leftLines[i] ?? " ".repeat(leftWidth)}${vBar}${rightLines[i] ?? " ".repeat(rightWidth)}`,
        );
      }
    } else {
      this.listRowMap = [];
      bodyLines.push(...this.renderActionPanel(totalWidth, bodyHeight));
    }

    // Shortcut/status row.
    const shortcutHint =
      `ctrl+s sort · ctrl+n named · ctrl+d delete · ctrl+p path (${this.showPath ? "on" : "off"}) · ctrl+r rename`;
    const hintLine = fitWidth(`  ${this.fg("dim", shortcutHint)}`, totalWidth);

    // Bottom footer row.
    const totalInScope = this.scope === "global" ? this.globalCount : this.projectCount;
    const footerLeft = this.fg("dim", `  grok-pi · /resume · ${this.hits.length}/${totalInScope}`);
    const footerRight = this.fg(
      "dim",
      `↑↓ select · Tab filter · PgUp/PgDn preview · Enter ${this.confirmLabel} · Esc close  `,
    );
    const footerGap = Math.max(1, totalWidth - visibleWidth(footerLeft) - visibleWidth(footerRight));
    const footer = fitWidth(`${footerLeft}${" ".repeat(footerGap)}${footerRight}`, totalWidth);

    return [row0, row1, row2, ...bodyLines, hintLine, footer];
  }

  render(width: number): string[] {
    if (this.fullscreen) {
      return this.renderFullscreen(width);
    }
    const termRows = this.getRows();
    const targetRows = Math.max(16, Math.floor(termRows * 0.9));
    if (this.mode !== "list") {
      this.listRowMap = [];
      return this.renderActionPanel(width, targetRows);
    }
    const geometry = computeSplitGeometry({
      width,
      rows: termRows,
      leftRatio: 0.4,
      hasSearchBar: true,
      heightRatio: 0.9,
      maxRows: targetRows,
    });
    this.dividerX = geometry.dividerX;
    this.bodyStartY = geometry.bodyStartY;
    this.bodyHeight = geometry.bodyHeight;

    const frame = renderSplitOverlayFrame({
      geometry,
      titlePrefix: ` ${this.bold("Sessions")} ${this.fg("dim", this.titleBadge)}  `,
      pills: [
        { id: "global", label: `[Global ${this.globalCount}]`, active: this.scope === "global" },
        { id: "project", label: `[Project ${this.projectCount}]`, active: this.scope === "project" },
      ],
      search: {
        promptPrefix: ` ${this.fg("accent", "Search:")} `,
        query: this.query,
        placeholder: "type to search user/agent messages, project, model, id...",
        rightLabel: `${this.statusMessage?.text ?? `${this.searchPending ? "searching · " : ""}${this.hits.length}/${this.scope === "global" ? this.globalCount : this.projectCount} · User & Agent only `}`,
      },
      leftLines: this.renderLeftPane(geometry.leftWidth, geometry.bodyHeight),
      rightLines: this.renderRightPane(geometry.rightWidth, geometry.bodyHeight),
      footerHints: [
        { key: "↑↓/Wheel", label: "select" },
        { key: "Tab/Click", label: "Global/Project" },
        { key: "PgUp/PgDn/Wheel", label: "preview" },
        { key: "Ctrl+S/N", label: "sort/named" },
        { key: "Ctrl+D/P/R", label: "delete/path/rename" },
        { key: "Enter/2×Click", label: this.confirmLabel },
        { key: "Esc", label: "close" },
      ],
      theme: this.theme,
    });
    this.pillRects = frame.pillRects;
    this.closeBtnRect = frame.closeRect;
    return frame.lines;
  }
}

export function createSessionPickerTui(
  tui: { requestRender: () => void },
  theme: TuiTheme,
  options: SessionPickerTuiOptions,
  done: (selected: SessionRecord | undefined) => void,
): SessionPickerTui {
  return new SessionPickerTui(tui, theme, options, done);
}
