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
import { fitWidth, type SessionPickerTheme } from "./session-picker-tui.ts";
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
  /** Optional injected runners for deterministic unit testing. */
  listFiles?: (rootDir: string, showHidden: boolean) => FileSearchEntry[];
  runGrep?: (rootDir: string, query: string, showHidden: boolean) => GrepHit[];
  loadPreview?: (
    rootDir: string,
    relPath: string,
    opts: { showHidden: boolean; nerdIcons: boolean },
  ) => FilePreviewResult;
  getRows?: () => number;
}

interface ClickRegion {
  x1: number;
  x2: number;
  y: number;
}

function highlightTerms(text: string, query: string, highlight: (s: string) => string): string {
  const terms = query
    .trim()
    .split(/\s+/)
    .filter((t) => t.length >= 2);
  if (terms.length === 0) return text;
  const escaped = terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  const re = new RegExp(`(${escaped.join("|")})`, "gi");
  return text.replace(re, (match) => highlight(match));
}

export class FilePickerTui implements Component {
  private readonly rootDir: string;
  private readonly tui: { requestRender: () => void };
  private readonly theme: SessionPickerTheme;
  private readonly done: (selected: FilePickerSelection | undefined) => void;
  private readonly listFilesFn: (rootDir: string, showHidden: boolean) => FileSearchEntry[];
  private readonly runGrepFn: (rootDir: string, query: string, showHidden: boolean) => GrepHit[];
  private readonly loadPreviewFn: (
    rootDir: string,
    relPath: string,
    opts: { showHidden: boolean; nerdIcons: boolean },
  ) => FilePreviewResult;
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

  // Selection & scroll state
  private selectedIndex = 0;
  private listScroll = 0;
  private previewScroll = 0;
  private lastPreviewKey = "";
  private lastPreviewLinesCount = 0;
  private readonly previewCache = new Map<string, FilePreviewResult>();

  // Layout geometry captured per frame for mouse hit-testing
  private dividerX = 43;
  private bodyStartY = 5;
  private bodyHeight = 14;
  private previewVisibleRows = 10;
  private treePillRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private filesPillRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private grepPillRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private hiddenPillRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private nerdPillRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private closeBtnRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private listRowMap: Array<{ y1: number; y2: number; index: number }> = [];

  constructor(
    tui: { requestRender: () => void },
    theme: SessionPickerTheme,
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
    this.getRows = options.getRows ?? (() => 28);

    this.refreshActiveMode();
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

  private refreshActiveMode(): void {
    if (this.mode === "tree") {
      this.treeNodes = buildVisibleTreeNodes(this.rootDir, this.expandedDirs, {
        showHidden: this.showHidden,
      });
    } else if (this.mode === "files") {
      this.fileHits = filterFileEntries(this.ensureFdFiles(), this.query);
    } else {
      const grepKey = `${this.showHidden ? "1" : "0"}:${this.query}`;
      if (grepKey !== this.lastGrepKey) {
        this.lastGrepKey = grepKey;
        this.grepHits = this.query.trim()
          ? this.runGrepFn(this.rootDir, this.query, this.showHidden)
          : [];
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
    this.lastGrepKey = "";
    this.refreshActiveMode();
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
      this.done(undefined);
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
      }
      this.done(this.getCurrentSelection());
      return;
    }
    // Space in Tree mode confirms even a directory as a whole-folder `@dir/` reference.
    if (data === " " && this.mode === "tree" && this.query.length === 0) {
      this.done(this.getCurrentSelection());
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

    const hitRegion = (r: ClickRegion) => y === r.y && x >= r.x1 && x <= r.x2;

    if (type === "press" && event.button === "left") {
      if (hitRegion(this.closeBtnRect)) {
        this.done(undefined);
        return { handled: true, render: false };
      }
      if (hitRegion(this.treePillRect)) {
        this.setMode("tree");
        return { handled: true, render: true };
      }
      if (hitRegion(this.filesPillRect)) {
        this.setMode("files");
        return { handled: true, render: true };
      }
      if (hitRegion(this.grepPillRect)) {
        this.setMode("grep");
        return { handled: true, render: true };
      }
      if (hitRegion(this.hiddenPillRect)) {
        this.toggleHidden();
        return { handled: true, render: true };
      }
      if (hitRegion(this.nerdPillRect)) {
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
            this.done(this.getCurrentSelection());
            return { handled: true, render: false };
          }
        }
      }
    }

    return { handled: true, render: false };
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
        this.mode === "grep" && !this.query.trim()
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
        : `${preview.sizeLabel}  ${this.fg("dim", "Lines:")} ${preview.lineCount}${preview.truncated ? "+" : ""}  ${this.fg("dim", "Updated:")} ${preview.modified}${lineTag}`;

    const metaLines: string[] = [
      pad(`${icon} ${this.bold(this.fg("accent", selection.title))}`),
      pad(`${this.fg("dim", "Info:")} ${detailText}`),
    ];

    const divider = this.fg("border", "─".repeat(innerW));
    const headerBlock = [...metaLines, divider];
    const previewHeight = Math.max(1, height - headerBlock.length);
    this.previewVisibleRows = previewHeight;

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
      for (let i = 0; i < preview.lines.length; i++) {
        const lineNo = i + 1;
        const isMatch = selection.line === lineNo;
        const numStr = String(lineNo).padStart(gutterW, " ");
        const prefix = isMatch
          ? `${this.fg("warning", "▸")}${this.bold(this.fg("warning", numStr))} ${this.fg("border", "│")} `
          : ` ${this.fg("dim", numStr)} ${this.fg("border", "│")} `;
        const raw = preview.lines[i]!.replace(/\t/g, "  ");
        const content =
          this.mode === "grep" && this.query.trim()
            ? highlightTerms(raw, this.query, (m) => `\x1b[7m${m}\x1b[27m`)
            : raw;
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
    const totalW = Math.max(60, Math.floor(width));
    const innerW = totalW - 2;

    // Telescope horizontal proportion: ~42% list/tree, ~58% preview.
    const leftW = Math.max(26, Math.min(innerW - 28, Math.floor(innerW * 0.42)));
    const rightW = Math.max(26, innerW - leftW - 1);
    this.dividerX = 1 + leftW;

    const totalRows = Math.max(16, Math.min(34, Math.floor(this.getRows() * 0.84)));
    const bodyH = Math.max(8, totalRows - 8);
    this.bodyStartY = 5;
    this.bodyHeight = bodyH;

    const b = (s: string) => this.fg("border", s);
    const topBorder = b(`╭${"─".repeat(innerW)}╮`);
    const midRule = b(`├${"─".repeat(innerW)}┤`);
    const splitTop = b(`├${"─".repeat(leftW)}┬${"─".repeat(rightW)}┤`);
    const splitBottom = b(`├${"─".repeat(leftW)}┴${"─".repeat(rightW)}┤`);
    const bottomBorder = b(`╰${"─".repeat(innerW)}╯`);

    // Row 1: Header with mode pills + Hidden + Nerd toggles + Close button.
    const rootName = path.basename(this.rootDir) || this.rootDir;
    const titlePrefix = ` ${this.bold(truncateToWidth(rootName, 18, "…"))}  `;
    const treeText = "[Tree]";
    const filesText = "[Files·fd]";
    const grepText = "[Grep·rg]";
    const hiddenText = this.showHidden ? "[Hidden:on]" : "[Hidden]";
    const nerdText = this.nerdIcons ? "[Nerd:on]" : "[Nerd]";
    const closeText = "[✕]";

    const stylePill = (text: string, active: boolean) =>
      active ? this.bold(this.fg("accent", text)) : this.fg("dim", text);

    let col = 1 + visibleWidth(titlePrefix);
    const place = (text: string): ClickRegion => {
      const start = col;
      const end = start + visibleWidth(text) - 1;
      col = end + 2;
      return { x1: start, x2: end, y: 1 };
    };
    this.treePillRect = place(treeText);
    this.filesPillRect = place(filesText);
    this.grepPillRect = place(grepText);
    this.hiddenPillRect = place(hiddenText);
    this.nerdPillRect = place(nerdText);

    const leftHeader =
      `${titlePrefix}${stylePill(treeText, this.mode === "tree")} ` +
      `${stylePill(filesText, this.mode === "files")} ` +
      `${stylePill(grepText, this.mode === "grep")} ` +
      `${stylePill(hiddenText, this.showHidden)} ` +
      `${stylePill(nerdText, this.nerdIcons)}`;
    const closeW = visibleWidth(closeText);
    const headerGap = Math.max(1, innerW - visibleWidth(leftHeader) - closeW - 1);
    const closeStart = 1 + innerW - closeW - 1;
    this.closeBtnRect = { x1: closeStart, x2: closeStart + closeW - 1, y: 1 };
    const headerLine = `${b("│")}${fitWidth(`${leftHeader}${" ".repeat(headerGap)}${this.fg("dim", closeText)} `, innerW)}${b("│")}`;

    // Row 3: Telescope-style search prompt (`` when Nerd icons enabled).
    const promptGlyph = this.nerdIcons ? " " : "> ";
    const modeLabel =
      this.mode === "tree" ? "Tree" : this.mode === "files" ? "fd" : "rg";
    const countLabel = `${this.itemCount()} ${modeLabel} `;
    const placeholder =
      this.mode === "grep"
        ? "rg full-text search across workspace..."
        : this.mode === "files"
          ? "fd smart file filter..."
          : "type to jump into fd smart search, ←/→ expand dirs...";
    const queryDisplay = this.query
      ? `${this.query}\x1b[7m \x1b[27m`
      : `\x1b[7m \x1b[27m${this.fg("dim", placeholder)}`;
    const searchBudget = Math.max(10, innerW - visibleWidth(countLabel) - 1);
    const searchLeft = fitWidth(` ${this.fg("accent", promptGlyph)}${queryDisplay}`, searchBudget);
    const searchLine = `${b("│")}${fitWidth(`${searchLeft} ${this.fg("dim", countLabel)}`, innerW)}${b("│")}`;

    // Body rows.
    const leftLines = this.renderLeftPane(leftW, bodyH);
    const rightLines = this.renderRightPane(rightW, bodyH);
    const bodyLines: string[] = [];
    for (let i = 0; i < bodyH; i++) {
      bodyLines.push(
        `${b("│")}${leftLines[i] ?? " ".repeat(leftW)}${b("│")}${rightLines[i] ?? " ".repeat(rightW)}${b("│")}`,
      );
    }

    // Footer hints.
    const footerText =
      ` ${this.fg("accent", "Tab")}${this.fg("dim", ":Tree/fd/rg  ")}` +
      `${this.fg("accent", "←→")}${this.fg("dim", ":fold  ")}` +
      `${this.fg("accent", "Ctrl+H")}${this.fg("dim", ":hidden  ")}` +
      `${this.fg("accent", "Ctrl+Y")}${this.fg("dim", ":nerd  ")}` +
      `${this.fg("accent", "Enter/2×Click")}${this.fg("dim", ":insert  ")}` +
      `${this.fg("accent", "Esc")}${this.fg("dim", ":close")}`;
    const footerLine = `${b("│")}${fitWidth(footerText, innerW)}${b("│")}`;

    return [
      topBorder,
      headerLine,
      midRule,
      searchLine,
      splitTop,
      ...bodyLines,
      splitBottom,
      footerLine,
      bottomBorder,
    ];
  }
}

export function createFilePickerTui(
  tui: { requestRender: () => void },
  theme: SessionPickerTheme,
  options: FilePickerTuiOptions,
  done: (selected: FilePickerSelection | undefined) => void,
): FilePickerTui {
  return new FilePickerTui(tui, theme, options, done);
}
