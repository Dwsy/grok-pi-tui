/**
 * Enhanced `@file` explorer TUI (`Tree` + `Files (fd)` + `Grep (rg)` + Preview).
 *
 * Modeled after NvChad's `nvim-tree.lua` + `nvim-web-devicons` + `telescope.nvim`:
 * - Rounded box border (`╭─╮│╰─╯`) with horizontal split (`~42%` left / `~58%` right)
 * - Mode switcher pills: `[Tree]` / `[Files·fd]` / `[Grep·rg]` + `[Hidden]` + `[Nerd]` + `[✕]`
 * - Tree mode: lazy-expanded directory tree with indent markers (`│`/`├`/`└`) & Nerd icons
 * - Files mode: cached `fd` workspace index with instant multi-term fuzzy ranking
 * - Grep mode: `rg --vimgrep --smart-case` full-text search with auto-centered line preview
 * - Right pane: metadata card + line-numbered file/directory preview (binary-safe)
 * - Full keyboard + mouse support (click pills, click/double-click rows, wheel scroll)
 */

import path from "node:path";
import {
  matchesKey,
  truncateToWidth,
  visibleWidth,
  type Component,
  type TuiMouseEvent,
  type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import {
  computeSplitGeometry,
  detectTerminalImageProtocol,
  fitWidth,
  highlightTerms,
  hitRegion,
  renderHalfBlockPreview,
  renderSplitOverlayFrame,
  type ClickRegion,
  type RemoteTuiImageSpec,
  type TerminalImageCapability,
  type TuiTheme,
} from "../pi-grok-tui-kit/index.ts";
import {
  buildVisibleTreeNodes,
  detectNerdFontSupport,
  filterFileEntries,
  getFileIcon,
  grepWorkspaceWithRg,
  listWorkspaceFilesWithFd,
  loadFileOrDirPreview,
  type FilePickerMode,
  type FilePreviewResult,
  type FileSearchEntry,
  type GrepHit,
  type TreeNode,
} from "./file-data.ts";

export interface FilePickerSelection {
  relPath: string;
  isDir: boolean;
  line?: number;
  insertText: string;
  title: string;
}

export interface FilePickerTuiOptions {
  rootDir: string;
  initialQuery?: string;
  initialMode?: FilePickerMode;
  showHidden?: boolean;
  nerdIcons?: boolean;
  /** Debounce delay in ms for `rg` grep queries while typing (0 = synchronous). */
  grepDebounceMs?: number;
  /** Optional injected runners for deterministic unit testing. */
  listFiles?: (rootDir: string, showHidden: boolean) => FileSearchEntry[];
  runGrep?: (rootDir: string, query: string, showHidden: boolean) => GrepHit[];
  loadPreview?: (
    rootDir: string,
    relPath: string,
    opts: { showHidden: boolean; nerdIcons: boolean },
  ) => FilePreviewResult;
  /** Optional syntax highlighter override (defaults to Pi's `highlightCode`). */
  highlightFile?: (code: string, relPath: string) => string[] | undefined;
  /** Optional callback to emit/clear an HD image overlay rect in `grok-pi`. */
  onImageOverlay?: (spec: RemoteTuiImageSpec | undefined) => void;
  /** Optional env override for deterministic protocol detection in tests. */
  env?: NodeJS.ProcessEnv;
  getRows?: () => number;
}

type PiHighlighterModule = {
  getLanguageFromPath?: (filePath: string) => string | undefined;
  highlightCode?: (code: string, lang?: string) => string[];
};

let cachedPiHighlighter: PiHighlighterModule | null | undefined;

function loadPiHighlighter(): PiHighlighterModule | null {
  if (cachedPiHighlighter !== undefined) return cachedPiHighlighter;
  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    cachedPiHighlighter = require("@earendil-works/pi-coding-agent") as PiHighlighterModule;
  } catch {
    cachedPiHighlighter = null;
  }
  return cachedPiHighlighter;
}

export function createPiCodeHighlighter(
  mod?: PiHighlighterModule | null,
): (code: string, relPath: string) => string[] | undefined {
  return (code: string, relPath: string) => {
    try {
      const resolved = mod ?? loadPiHighlighter();
      if (!resolved?.getLanguageFromPath || !resolved?.highlightCode) return undefined;
      const lang = resolved.getLanguageFromPath(relPath);
      if (!lang) return undefined;
      return resolved.highlightCode(code, lang);
    } catch {
      // Pi's global theme may be uninitialized in standalone unit tests.
      return undefined;
    }
  };
}

const defaultHighlightWithPi = createPiCodeHighlighter();

function lineMatchesAnyTerm(line: string, query: string): boolean {
  const terms = query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);
  if (terms.length === 0) return false;
  const lower = line.toLowerCase();
  return terms.some((t) => lower.includes(t));
}

export class FilePickerTui implements Component {
  private readonly rootDir: string;
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly done: (selected: FilePickerSelection | undefined) => void;
  private readonly listFilesFn: (rootDir: string, showHidden: boolean) => FileSearchEntry[];
  private readonly runGrepFn: (rootDir: string, query: string, showHidden: boolean) => GrepHit[];
  private readonly loadPreviewFn: (
    rootDir: string,
    relPath: string,
    opts: { showHidden: boolean; nerdIcons: boolean },
  ) => FilePreviewResult;
  private readonly highlightFileFn: (code: string, relPath: string) => string[] | undefined;
  private readonly grepDebounceMs: number;
  private readonly onImageOverlay?: (spec: RemoteTuiImageSpec | undefined) => void;
  private readonly imageCap: TerminalImageCapability;
  private readonly getRows: () => number;

  private mode: FilePickerMode;
  private query: string;
  private showHidden: boolean;
  private nerdIcons: boolean;

  // Tree state
  private readonly expandedDirs = new Set<string>();
  private treeNodes: TreeNode[] = [];

  // fd Files state (cached per showHidden setting so keystrokes stay <2ms)
  private cachedFdFiles: { showHidden: boolean; entries: FileSearchEntry[] } | null = null;
  private fileHits: FileSearchEntry[] = [];

  // rg Grep state
  private grepHits: GrepHit[] = [];
  private lastGrepKey = "";
  private grepTimer: ReturnType<typeof setTimeout> | null = null;
  private grepPending = false;

  // Selection & scroll state
  private selectedIndex = 0;
  private listScroll = 0;
  private previewScroll = 0;
  private lastPreviewKey = "";
  private lastPreviewLinesCount = 0;
  private readonly previewCache = new Map<string, FilePreviewResult>();
  private readonly highlightCache = new Map<string, string[] | null>();

  // Layout geometry captured per frame for mouse hit-testing
  private dividerX = 43;
  private bodyStartY = 5;
  private bodyHeight = 14;
  private previewVisibleRows = 10;
  private lastEmittedImageKey = "";
  private pillRects = new Map<string, ClickRegion>();
  private closeBtnRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private listRowMap: Array<{ y1: number; y2: number; index: number }> = [];

  constructor(
    tui: { requestRender: () => void },
    theme: TuiTheme,
    options: FilePickerTuiOptions,
    done: (selected: FilePickerSelection | undefined) => void,
  ) {
    this.tui = tui;
    this.theme = theme;
    this.rootDir = options.rootDir;
    this.query = options.initialQuery ?? "";
    this.mode = options.initialMode ?? (this.query.trim() ? "files" : "tree");
    this.showHidden = options.showHidden ?? false;
    this.nerdIcons = options.nerdIcons ?? detectNerdFontSupport();
    this.done = done;
    this.listFilesFn =
      options.listFiles ?? ((root, hidden) => listWorkspaceFilesWithFd(root, { showHidden: hidden }));
    this.runGrepFn =
      options.runGrep ?? ((root, q, hidden) => grepWorkspaceWithRg(root, q, { showHidden: hidden }));
    this.loadPreviewFn = options.loadPreview ?? loadFileOrDirPreview;
    this.highlightFileFn = options.highlightFile ?? defaultHighlightWithPi;
    this.grepDebounceMs = Math.max(0, options.grepDebounceMs ?? 0);
    this.onImageOverlay = options.onImageOverlay;
    this.imageCap = detectTerminalImageProtocol(options.env);
    this.getRows = options.getRows ?? (() => 28);

    this.refreshActiveMode(true);
  }

  private syncImageOverlay(spec: RemoteTuiImageSpec | undefined): void {
    if (!this.onImageOverlay) return;
    const key = spec
      ? `${spec.path}:${spec.x},${spec.y},${spec.width}x${spec.height}`
      : "";
    if (key === this.lastEmittedImageKey) return;
    this.lastEmittedImageKey = key;
    this.onImageOverlay(spec);
  }

  private clearGrepTimer(): void {
    if (this.grepTimer !== null) {
      clearTimeout(this.grepTimer);
      this.grepTimer = null;
    }
    this.grepPending = false;
  }

  private executeGrepNow(): void {
    this.clearGrepTimer();
    const grepKey = `${this.showHidden ? "1" : "0"}:${this.query}`;
    if (grepKey === this.lastGrepKey) return;
    this.lastGrepKey = grepKey;
    this.grepHits = this.query.trim()
      ? this.runGrepFn(this.rootDir, this.query, this.showHidden)
      : [];
    this.clampSelection();
  }

  private finish(selected: FilePickerSelection | undefined): void {
    this.clearGrepTimer();
    this.syncImageOverlay(undefined);
    this.done(selected);
  }

  getMode(): FilePickerMode {
    return this.mode;
  }

  getQuery(): string {
    return this.query;
  }

  isShowingHidden(): boolean {
    return this.showHidden;
  }

  isUsingNerdIcons(): boolean {
    return this.nerdIcons;
  }

  invalidate(): void {
    // Re-renders directly from live state on every frame.
  }

  private bold(text: string): string {
    return this.theme.bold ? this.theme.bold(text) : `\x1b[1m${text}\x1b[0m`;
  }

  private fg(color: string, text: string): string {
    return this.theme.fg(color, text);
  }

  private styleIcon(name: string, opts: { isDir?: boolean; expanded?: boolean; isSymlink?: boolean }): string {
    const spec = getFileIcon(name, { ...opts, nerdIcons: this.nerdIcons });
    if (spec.color === "default") return spec.glyph;
    return this.fg(spec.color, spec.glyph);
  }

  private ensureFdFiles(): FileSearchEntry[] {
    if (!this.cachedFdFiles || this.cachedFdFiles.showHidden !== this.showHidden) {
      this.cachedFdFiles = {
        showHidden: this.showHidden,
        entries: this.listFilesFn(this.rootDir, this.showHidden),
      };
    }
    return this.cachedFdFiles.entries;
  }

  private refreshActiveMode(immediate = false): void {
    if (this.mode === "tree") {
      this.clearGrepTimer();
      this.treeNodes = buildVisibleTreeNodes(this.rootDir, this.expandedDirs, {
        showHidden: this.showHidden,
      });
    } else if (this.mode === "files") {
      this.clearGrepTimer();
      this.fileHits = filterFileEntries(this.ensureFdFiles(), this.query);
    } else {
      if (!this.query.trim()) {
        this.clearGrepTimer();
        this.lastGrepKey = "";
        this.grepHits = [];
      } else if (immediate || this.grepDebounceMs <= 0) {
        this.executeGrepNow();
      } else {
        const grepKey = `${this.showHidden ? "1" : "0"}:${this.query}`;
        if (grepKey !== this.lastGrepKey) {
          if (this.grepTimer !== null) {
            clearTimeout(this.grepTimer);
          }
          this.grepPending = true;
          this.grepTimer = setTimeout(() => {
            this.grepTimer = null;
            this.grepPending = false;
            if (this.mode !== "grep") return;
            this.executeGrepNow();
            this.tui.requestRender();
          }, this.grepDebounceMs);
        }
      }
    }
    this.clampSelection();
  }

  private itemCount(): number {
    if (this.mode === "tree") return this.treeNodes.length;
    if (this.mode === "files") return this.fileHits.length;
    return this.grepHits.length;
  }

  private rowsPerItem(): number {
    return this.mode === "grep" ? 2 : 1;
  }

  private itemsPerPage(): number {
    return Math.max(1, Math.floor(this.bodyHeight / this.rowsPerItem()));
  }

  private clampSelection(): void {
    const count = this.itemCount();
    const page = this.itemsPerPage();
    if (count === 0) {
      this.selectedIndex = 0;
      this.listScroll = 0;
      return;
    }
    this.selectedIndex = Math.max(0, Math.min(count - 1, this.selectedIndex));
    if (this.selectedIndex < this.listScroll) {
      this.listScroll = this.selectedIndex;
    } else if (this.selectedIndex >= this.listScroll + page) {
      this.listScroll = this.selectedIndex - page + 1;
    }
    const maxScroll = Math.max(0, count - page);
    this.listScroll = Math.max(0, Math.min(maxScroll, this.listScroll));
  }

  private setMode(next: FilePickerMode): void {
    if (this.mode === next) return;
    this.mode = next;
    this.selectedIndex = 0;
    this.listScroll = 0;
    this.previewScroll = 0;
    this.lastPreviewKey = "";
    this.refreshActiveMode();
  }

  private cycleMode(): void {
    const order: FilePickerMode[] = ["tree", "files", "grep"];
    const next = order[(order.indexOf(this.mode) + 1) % order.length]!;
    this.setMode(next);
  }

  private toggleHidden(): void {
    this.showHidden = !this.showHidden;
    this.cachedFdFiles = null;
    this.previewCache.clear();
    this.highlightCache.clear();
    this.lastGrepKey = "";
    this.refreshActiveMode(true);
  }

  private toggleNerdIcons(): void {
    this.nerdIcons = !this.nerdIcons;
    this.previewCache.clear();
  }

  private moveSelection(delta: number): void {
    const count = this.itemCount();
    if (count === 0) return;
    const prev = this.selectedIndex;
    this.selectedIndex = Math.max(0, Math.min(count - 1, this.selectedIndex + delta));
    if (this.selectedIndex !== prev) {
      this.previewScroll = 0;
      this.lastPreviewKey = "";
    }
    this.clampSelection();
  }

  private scrollPreview(delta: number): void {
    const maxScroll = Math.max(0, this.lastPreviewLinesCount - this.previewVisibleRows);
    this.previewScroll = Math.max(0, Math.min(maxScroll, this.previewScroll + delta));
  }

  /** Build the current selection payload (for Enter / double-click / tests). */
  getCurrentSelection(): FilePickerSelection | undefined {
    if (this.mode === "tree") {
      const node = this.treeNodes[this.selectedIndex];
      if (!node) return undefined;
      const insertPath = node.isDir ? `${node.relPath}/` : node.relPath;
      return {
        relPath: node.relPath,
        isDir: node.isDir,
        insertText: `@${insertPath}`,
        title: insertPath,
      };
    }
    if (this.mode === "files") {
      const entry = this.fileHits[this.selectedIndex];
      if (!entry) return undefined;
      const insertPath = entry.isDir ? `${entry.relPath}/` : entry.relPath;
      return {
        relPath: entry.relPath,
        isDir: entry.isDir,
        insertText: `@${insertPath}`,
        title: insertPath,
      };
    }
    const hit = this.grepHits[this.selectedIndex];
    if (!hit) return undefined;
    const target = `${hit.relPath}:${hit.line}`;
    return {
      relPath: hit.relPath,
      isDir: false,
      line: hit.line,
      insertText: `@${target}`,
      title: target,
    };
  }

  private toggleTreeDir(node: TreeNode, force?: boolean): void {
    if (!node.isDir) return;
    const shouldExpand = force ?? !this.expandedDirs.has(node.relPath);
    if (shouldExpand) {
      this.expandedDirs.add(node.relPath);
    } else {
      this.expandedDirs.delete(node.relPath);
    }
    this.refreshActiveMode();
  }

  handleInput(data: string): void {
    if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
      this.finish(undefined);
      return;
    }
    if (matchesKey(data, "tab")) {
      this.cycleMode();
      return;
    }
    if (matchesKey(data, "ctrl+h")) {
      this.toggleHidden();
      return;
    }
    if (matchesKey(data, "ctrl+y")) {
      this.toggleNerdIcons();
      return;
    }
    if (matchesKey(data, "up") || matchesKey(data, "ctrl+p")) {
      this.moveSelection(-1);
      return;
    }
    if (matchesKey(data, "down") || matchesKey(data, "ctrl+n")) {
      this.moveSelection(1);
      return;
    }
    if (matchesKey(data, "pageUp") || matchesKey(data, "ctrl+u")) {
      this.scrollPreview(-Math.max(3, Math.floor(this.previewVisibleRows / 2)));
      return;
    }
    if (matchesKey(data, "pageDown") || matchesKey(data, "ctrl+d")) {
      this.scrollPreview(Math.max(3, Math.floor(this.previewVisibleRows / 2)));
      return;
    }
    if (matchesKey(data, "right") && this.mode === "tree") {
      const node = this.treeNodes[this.selectedIndex];
      if (node?.isDir) this.toggleTreeDir(node, true);
      return;
    }
    if (matchesKey(data, "left") && this.mode === "tree") {
      const node = this.treeNodes[this.selectedIndex];
      if (!node) return;
      if (node.isDir && node.expanded) {
        this.toggleTreeDir(node, false);
      } else if (node.relPath.includes("/")) {
        const parentRel = path.posix.dirname(node.relPath);
        const parentIdx = this.treeNodes.findIndex((n) => n.relPath === parentRel);
        if (parentIdx >= 0) {
          this.selectedIndex = parentIdx;
          this.clampSelection();
        }
      }
      return;
    }
    if (matchesKey(data, "enter") || matchesKey(data, "return")) {
      if (this.mode === "tree") {
        const node = this.treeNodes[this.selectedIndex];
        if (node?.isDir) {
          this.toggleTreeDir(node);
          return;
        }
      } else if (this.mode === "grep" && this.grepPending) {
        this.executeGrepNow();
      }
      this.finish(this.getCurrentSelection());
      return;
    }
    // Space in Tree mode confirms even a directory as a whole-folder `@dir/` reference.
    if (data === " " && this.mode === "tree" && this.query.length === 0) {
      this.finish(this.getCurrentSelection());
      return;
    }
    if (matchesKey(data, "ctrl+w") || matchesKey(data, "ctrl+l")) {
      if (this.query.length > 0) {
        this.query = "";
        this.refreshActiveMode();
      }
      return;
    }
    if (matchesKey(data, "backspace")) {
      if (this.query.length > 0) {
        this.query = Array.from(this.query).slice(0, -1).join("");
        this.selectedIndex = 0;
        this.listScroll = 0;
        this.refreshActiveMode();
      }
      return;
    }

    let printable = data;
    if (printable.startsWith("\x1b[200~") && printable.endsWith("\x1b[201~")) {
      printable = printable.slice(6, -6).replace(/[\r\n]+/g, " ");
    }
    if (!printable.startsWith("\x1b") && !/[\x00-\x1f\x7f]/.test(printable)) {
      // Typing directly from Tree mode jumps straight into fast `fd` Files search.
      if (this.mode === "tree") {
        this.mode = "files";
      }
      this.query += printable;
      this.selectedIndex = 0;
      this.listScroll = 0;
      this.previewScroll = 0;
      this.lastPreviewKey = "";
      this.refreshActiveMode();
    }
  }

  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    const { x, y, type } = event;

    if (type === "wheel") {
      const delta = (event.wheelDelta ?? 0) > 0 ? 1 : -1;
      if (y >= this.bodyStartY && y < this.bodyStartY + this.bodyHeight) {
        if (x < this.dividerX) {
          this.moveSelection(delta);
        } else {
          this.scrollPreview(delta * 3);
        }
        return { handled: true, render: true };
      }
      return { handled: true, render: false };
    }

    if (type === "press" && event.button === "left") {
      if (hitRegion(this.closeBtnRect, x, y)) {
        this.finish(undefined);
        return { handled: true, render: false };
      }
      if (hitRegion(this.pillRects.get("tree"), x, y)) {
        this.setMode("tree");
        return { handled: true, render: true };
      }
      if (hitRegion(this.pillRects.get("files"), x, y)) {
        this.setMode("files");
        return { handled: true, render: true };
      }
      if (hitRegion(this.pillRects.get("grep"), x, y)) {
        this.setMode("grep");
        return { handled: true, render: true };
      }
      if (hitRegion(this.pillRects.get("hidden"), x, y)) {
        this.toggleHidden();
        return { handled: true, render: true };
      }
      if (hitRegion(this.pillRects.get("nerd"), x, y)) {
        this.toggleNerdIcons();
        return { handled: true, render: true };
      }
      if (x >= 1 && x < this.dividerX) {
        for (const row of this.listRowMap) {
          if (y >= row.y1 && y <= row.y2) {
            const wasSelected = this.selectedIndex === row.index;
            this.selectedIndex = row.index;
            this.previewScroll = 0;
            this.lastPreviewKey = "";
            this.clampSelection();
            if (this.mode === "tree") {
              const node = this.treeNodes[row.index];
              if (node?.isDir && wasSelected) {
                this.toggleTreeDir(node);
              }
            }
            return { handled: true, render: true };
          }
        }
      }
      return { handled: true, render: false };
    }

    if (type === "click" && event.button === "left" && (event.clickCount ?? 1) >= 2) {
      if (x >= 1 && x < this.dividerX) {
        for (const row of this.listRowMap) {
          if (y >= row.y1 && y <= row.y2) {
            this.selectedIndex = row.index;
            if (this.mode === "tree") {
              const node = this.treeNodes[row.index];
              if (node?.isDir) {
                this.toggleTreeDir(node);
                return { handled: true, render: true };
              }
            }
            this.finish(this.getCurrentSelection());
            return { handled: true, render: false };
          }
        }
      }
    }

    return { handled: true, render: false };
  }

  private getHighlightedLines(relPath: string, rawLines: readonly string[]): string[] | null {
    const key = `${relPath}:${rawLines.length}:${rawLines[0] ?? ""}`;
    if (this.highlightCache.has(key)) {
      return this.highlightCache.get(key) ?? null;
    }
    const normalized = rawLines.map((l) => l.replace(/\t/g, "  "));
    const highlighted = this.highlightFileFn(normalized.join("\n"), relPath);
    const result = highlighted && highlighted.length === rawLines.length ? highlighted : null;
    this.highlightCache.set(key, result);
    return result;
  }

  private getPreview(relPath: string): FilePreviewResult {
    const key = `${this.showHidden ? "1" : "0"}:${this.nerdIcons ? "1" : "0"}:${relPath}`;
    const cached = this.previewCache.get(key);
    if (cached) return cached;
    const loaded = this.loadPreviewFn(this.rootDir, relPath, {
      showHidden: this.showHidden,
      nerdIcons: this.nerdIcons,
    });
    this.previewCache.set(key, loaded);
    return loaded;
  }

  private renderLeftPane(innerW: number, height: number): string[] {
    this.listRowMap = [];
    this.clampSelection();
    const count = this.itemCount();

    if (count === 0) {
      const emptyMsg =
        this.mode === "grep" && this.grepPending
          ? " Searching workspace with rg..."
          : this.mode === "grep" && !this.query.trim()
            ? " Type to grep workspace with rg..."
            : " No matching files";
      const lines = ["", this.fg("dim", emptyMsg)];
      while (lines.length < height) lines.push("");
      return lines.slice(0, height).map((l) => fitWidth(l, innerW));
    }

    const lines: string[] = [];
    const page = this.itemsPerPage();

    if (this.mode === "tree") {
      for (let slot = 0; slot < page; slot++) {
        const idx = this.listScroll + slot;
        if (idx >= this.treeNodes.length) break;
        const node = this.treeNodes[idx]!;
        const isSelected = idx === this.selectedIndex;
        const y = this.bodyStartY + lines.length;

        const marker = isSelected ? this.fg("accent", "▸") : " ";
        let indent = "";
        for (const hasNext of node.ancestorHasNext) {
          indent += hasNext ? this.fg("border", "│ ") : "  ";
        }
        if (node.depth > 0) {
          indent += this.fg("border", node.isLast ? "└─" : "├─");
        }
        const icon = this.styleIcon(node.name, {
          isDir: node.isDir,
          expanded: node.expanded,
          isSymlink: node.isSymlink,
        });
        const suffix = node.isDir ? "/" : "";
        const label = isSelected
          ? this.bold(this.fg("accent", `${node.name}${suffix}`))
          : node.isDir
            ? this.fg("accent", `${node.name}${suffix}`)
            : node.name;
        lines.push(fitWidth(`${marker}${indent}${icon} ${label}`, innerW));
        this.listRowMap.push({ y1: y, y2: y, index: idx });
      }
    } else if (this.mode === "files") {
      for (let slot = 0; slot < page; slot++) {
        const idx = this.listScroll + slot;
        if (idx >= this.fileHits.length) break;
        const entry = this.fileHits[idx]!;
        const isSelected = idx === this.selectedIndex;
        const y = this.bodyStartY + lines.length;

        const marker = isSelected ? this.fg("accent", "▸ ") : "  ";
        const icon = this.styleIcon(entry.name, { isDir: entry.isDir });
        const suffix = entry.isDir ? "/" : "";
        const pathLabel = `${entry.relPath}${suffix}`;
        const styled = isSelected
          ? this.bold(this.fg("accent", pathLabel))
          : entry.isDir
            ? this.fg("accent", pathLabel)
            : pathLabel;
        lines.push(fitWidth(`${marker}${icon} ${styled}`, innerW));
        this.listRowMap.push({ y1: y, y2: y, index: idx });
      }
    } else {
      for (let slot = 0; slot < page; slot++) {
        const idx = this.listScroll + slot;
        if (idx >= this.grepHits.length) break;
        const hit = this.grepHits[idx]!;
        const isSelected = idx === this.selectedIndex;
        const startY = this.bodyStartY + lines.length;

        const marker = isSelected ? this.fg("accent", "▸ ") : "  ";
        const icon = this.styleIcon(path.posix.basename(hit.relPath), { isDir: false });
        const loc = `${hit.relPath}:${hit.line}`;
        const locStyled = isSelected ? this.bold(this.fg("accent", loc)) : this.fg("accent", loc);
        lines.push(fitWidth(`${marker}${icon} ${locStyled}`, innerW));
        if (lines.length < height) {
          const snippet = highlightTerms(hit.text.trim(), this.query, (m) => `\x1b[7m${m}\x1b[27m`);
          lines.push(fitWidth(`   ${this.fg("dim", snippet)}`, innerW));
        }
        this.listRowMap.push({ y1: startY, y2: this.bodyStartY + lines.length - 1, index: idx });
      }
    }

    while (lines.length < height) {
      lines.push(" ".repeat(innerW));
    }
    return lines.slice(0, height);
  }

  private renderRightPane(innerW: number, height: number): string[] {
    const selection = this.getCurrentSelection();
    if (!selection) {
      this.syncImageOverlay(undefined);
      const empty = [
        "",
        this.fg("dim", "  No file selected."),
        this.fg("dim", "  Use Tree, Files (fd), or Grep (rg) to browse workspace."),
      ];
      while (empty.length < height) empty.push("");
      return empty.slice(0, height).map((l) => fitWidth(l, innerW));
    }

    const preview = this.getPreview(selection.relPath);
    const icon = this.styleIcon(path.posix.basename(selection.relPath), {
      isDir: selection.isDir,
      expanded: selection.isDir,
    });
    const pad = (s: string) => ` ${s}`;
    const lineTag = selection.line ? `  ${this.fg("warning", `Line ${selection.line}`)}` : "";
    const detailText =
      preview.kind === "dir"
        ? `${preview.dirSummary ?? preview.sizeLabel}  ${this.fg("dim", "Updated:")} ${preview.modified}`
        : preview.kind === "image"
          ? `${preview.imageMeta?.format ?? "IMG"}${preview.imageMeta && preview.imageMeta.width > 0 ? ` ${preview.imageMeta.width}×${preview.imageMeta.height}` : ""}  ${preview.sizeLabel}  ${this.fg("accent", this.onImageOverlay && this.imageCap.protocol !== "halfblock" ? this.imageCap.label : "Half-Block")}`
        : `${preview.sizeLabel}  ${this.fg("dim", "Lines:")} ${preview.lineCount}${preview.truncated ? "+" : ""}  ${this.fg("dim", "Updated:")} ${preview.modified}${lineTag}`;

    const metaLines: string[] = [
      pad(`${icon} ${this.bold(this.fg("accent", selection.title))}`),
      pad(`${this.fg("dim", "Info:")} ${detailText}`),
    ];

    const divider = this.fg("border", "─".repeat(innerW));
    const headerBlock = [...metaLines, divider];
    const previewHeight = Math.max(1, height - headerBlock.length);
    this.previewVisibleRows = previewHeight;

    if (preview.kind === "image" && preview.absPath) {
      const useNativeHd = Boolean(this.onImageOverlay && this.imageCap.protocol !== "halfblock");
      if (useNativeHd && innerW >= 6 && previewHeight >= 3) {
        this.syncImageOverlay({
          path: preview.absPath,
          x: this.dividerX + 1,
          y: this.bodyStartY + headerBlock.length,
          width: innerW,
          height: previewHeight,
        });
        this.lastPreviewLinesCount = 0;
        const blankCanvas = Array.from({ length: previewHeight }, () => "");
        return [...headerBlock, ...blankCanvas]
          .slice(0, height)
          .map((line) => fitWidth(line, innerW));
      }
      this.syncImageOverlay(undefined);
      const halfBlock = renderHalfBlockPreview(
        preview.absPath,
        Math.max(4, innerW - 2),
        Math.max(2, previewHeight),
        preview.imageMeta,
      );
      const imgLines = halfBlock
        ? halfBlock.map((row) => ` ${row}`)
        : preview.lines.map((l) => ` ${this.fg("dim", l)}`);
      this.lastPreviewLinesCount = imgLines.length;
      while (imgLines.length < previewHeight) imgLines.push("");
      return [...headerBlock, ...imgLines]
        .slice(0, height)
        .map((line) => fitWidth(line, innerW));
    }

    this.syncImageOverlay(undefined);

    // Auto-center the matched line when switching to a new grep hit.
    const previewKey = `${selection.relPath}:${selection.line ?? 0}`;
    if (previewKey !== this.lastPreviewKey) {
      this.lastPreviewKey = previewKey;
      if (selection.line && preview.kind === "file") {
        this.previewScroll = Math.max(0, selection.line - 1 - Math.floor(previewHeight / 3));
      } else {
        this.previewScroll = 0;
      }
    }

    const formattedLines: string[] = [];
    if (preview.kind === "file") {
      const gutterW = Math.max(3, String(preview.lines.length).length);
      const highlightedLines = this.getHighlightedLines(selection.relPath, preview.lines);
      const hasGrepQuery = this.mode === "grep" && this.query.trim().length > 0;
      for (let i = 0; i < preview.lines.length; i++) {
        const lineNo = i + 1;
        const isMatch = selection.line === lineNo;
        const numStr = String(lineNo).padStart(gutterW, " ");
        const prefix = isMatch
          ? `${this.fg("warning", "▸")}${this.bold(this.fg("warning", numStr))} ${this.fg("border", "│")} `
          : ` ${this.fg("dim", numStr)} ${this.fg("border", "│")} `;
        const raw = preview.lines[i]!.replace(/\t/g, "  ");
        const content =
          hasGrepQuery && lineMatchesAnyTerm(raw, this.query)
            ? highlightTerms(raw, this.query, (m) => `\x1b[7m${m}\x1b[27m`)
            : (highlightedLines?.[i] ?? raw);
        formattedLines.push(`${prefix}${content}`);
      }
      if (preview.truncated) {
        formattedLines.push(this.fg("dim", " … preview truncated"));
      }
    } else {
      for (const l of preview.lines) {
        formattedLines.push(` ${l}`);
      }
    }

    this.lastPreviewLinesCount = formattedLines.length;
    const maxScroll = Math.max(0, formattedLines.length - previewHeight);
    if (this.previewScroll > maxScroll) this.previewScroll = maxScroll;

    const visiblePreview = formattedLines.slice(
      this.previewScroll,
      this.previewScroll + previewHeight,
    );
    while (visiblePreview.length < previewHeight) visiblePreview.push("");

    return [...headerBlock, ...visiblePreview]
      .slice(0, height)
      .map((line) => fitWidth(line, innerW));
  }

  render(width: number): string[] {
    const termRows = this.getRows();
    const targetRows = Math.max(16, Math.floor(termRows * 0.9));
    const geometry = computeSplitGeometry({
      width,
      rows: termRows,
      leftRatio: 0.42,
      hasSearchBar: true,
      minWidth: 60,
      minLeftWidth: 26,
      minRightWidth: 26,
      heightRatio: 0.9,
      maxRows: targetRows,
    });
    this.dividerX = geometry.dividerX;
    this.bodyStartY = geometry.bodyStartY;
    this.bodyHeight = geometry.bodyHeight;

    const rootName = path.basename(this.rootDir) || this.rootDir;
    const promptGlyph = this.nerdIcons ? " " : "> ";
    const modeLabel = this.mode === "tree" ? "Tree" : this.mode === "files" ? "fd" : "rg";
    const placeholder =
      this.mode === "grep"
        ? "rg full-text search across workspace..."
        : this.mode === "files"
          ? "fd smart file filter..."
          : "type to jump into fd smart search, ←/→ expand dirs...";

    const frame = renderSplitOverlayFrame({
      geometry,
      titlePrefix: ` ${this.bold(truncateToWidth(rootName, 18, "…"))}  `,
      pills: [
        { id: "tree", label: "[Tree]", active: this.mode === "tree" },
        { id: "files", label: "[Files·fd]", active: this.mode === "files" },
        { id: "grep", label: "[Grep·rg]", active: this.mode === "grep" },
        { id: "hidden", label: this.showHidden ? "[Hidden:on]" : "[Hidden]", active: this.showHidden },
        { id: "nerd", label: this.nerdIcons ? "[Nerd:on]" : "[Nerd]", active: this.nerdIcons },
      ],
      search: {
        promptPrefix: ` ${this.fg("accent", promptGlyph)}`,
        query: this.query,
        placeholder,
        rightLabel:
          this.mode === "grep" && this.grepPending
            ? `searching… ${modeLabel} `
            : `${this.itemCount()} ${modeLabel} `,
      },
      leftLines: this.renderLeftPane(geometry.leftWidth, geometry.bodyHeight),
      rightLines: this.renderRightPane(geometry.rightWidth, geometry.bodyHeight),
      footerHints: [
        { key: "Tab", label: "Tree/fd/rg" },
        { key: "←→", label: "fold" },
        { key: "Ctrl+H", label: "hidden" },
        { key: "Ctrl+Y", label: "nerd" },
        { key: "Enter/2×Click", label: "insert" },
        { key: "Esc", label: "close" },
      ],
      theme: this.theme,
    });
    this.pillRects = frame.pillRects;
    this.closeBtnRect = frame.closeRect;
    return frame.lines;
  }
}

export function createFilePickerTui(
  tui: { requestRender: () => void },
  theme: TuiTheme,
  options: FilePickerTuiOptions,
  done: (selected: FilePickerSelection | undefined) => void,
): FilePickerTui {
  return new FilePickerTui(tui, theme, options, done);
}
