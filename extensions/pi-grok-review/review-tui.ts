/**
 * Minimal, bordered left/right split TUI for `/review-session` and `/review-message`.
 *
 * Shares Nerd Font icon & file-preview primitives from `../pi-grok-mentions/`
 * while deliberately stripping away the old Rust review modal's sprawling
 * keymap:
 * - ↑/↓ / Click / Wheel : select turn or changed file
 * - Tab / Click pills   : switch Tree ↔ List or Diff ↔ File preview
 * - Enter / c           : add or edit a review note on the selected file
 * - Ctrl+S / [Send]     : send all review notes to the agent as structured feedback
 * - Esc / [✕]           : cancel note input or close
 */

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
  detectNerdFontSupport,
  detectTerminalImageProtocol,
  fitWidth,
  getFileIcon,
  hitRegion,
  isImageFilePath,
  renderHalfBlockPreview,
  renderSplitOverlayFrame,
  RenderCoalescer,
  type ClickRegion,
  type RemoteTuiImageSpec,
  type TerminalImageCapability,
  type TuiTheme,
} from "../pi-grok-tui-kit/index.ts";
import { loadFileOrDirPreview, type FilePreviewResult } from "../pi-grok-mentions/file-data.ts";
import {
  buildReviewFileTree,
  type ReviewFileChange,
  type ReviewTreeRow,
  type ReviewTurn,
} from "./review-data.ts";

export type ReviewLaunchMode = "session" | "message";
export type ReviewListMode = "tree" | "list";
export type ReviewPreviewMode = "diff" | "file";

export interface ReviewComment {
  relPath: string;
  note: string;
}

export interface ReviewTuiResult {
  comments: ReviewComment[];
  formattedFeedback: string;
}

export interface ReviewTuiOptions {
  rootDir: string;
  mode: ReviewLaunchMode;
  /** Session-level aggregated files (used when `mode === "session"`). */
  files: readonly ReviewFileChange[];
  /** Turn-grouped files (used when `mode === "message"`). */
  turns?: readonly ReviewTurn[];
  nerdIcons?: boolean;
  loadPreview?: (
    rootDir: string,
    relPath: string,
    opts: { showHidden: boolean; nerdIcons: boolean },
  ) => FilePreviewResult;
  /** Optional callback to emit/clear an HD image overlay rect in `grok-pi`. */
  onImageOverlay?: (spec: RemoteTuiImageSpec | undefined) => void;
  /** Optional env override for deterministic protocol detection in tests. */
  env?: NodeJS.ProcessEnv;
  getRows?: () => number;
}

/** Format collected file comments into a concise, actionable prompt for the agent. */
export function formatReviewFeedback(comments: readonly ReviewComment[]): string {
  const lines = comments
    .filter((c) => c.note.trim().length > 0)
    .map((c) => `- @${c.relPath}: ${c.note.trim()}`);
  if (lines.length === 0) return "";
  return `Code review feedback:\n${lines.join("\n")}`;
}

export class ReviewTui implements Component {
  private readonly tui: { requestRender: () => void };
  private readonly rootDir: string;
  private readonly launchMode: ReviewLaunchMode;
  private readonly sessionFiles: readonly ReviewFileChange[];
  private readonly turns: readonly ReviewTurn[];
  private readonly theme: TuiTheme;
  private readonly done: (result: ReviewTuiResult | undefined) => void;
  private readonly loadPreviewFn: (
    rootDir: string,
    relPath: string,
    opts: { showHidden: boolean; nerdIcons: boolean },
  ) => FilePreviewResult;
  private readonly onImageOverlay?: (spec: RemoteTuiImageSpec | undefined) => void;
  private readonly imageCap: TerminalImageCapability;
  private readonly getRows: () => number;
  private readonly renderCoalescer: RenderCoalescer;

  private pickingTurn: boolean;
  private selectedTurnIndex = 0;
  private listMode: ReviewListMode = "tree";
  private previewMode: ReviewPreviewMode = "diff";
  private nerdIcons: boolean;

  private activeFiles: readonly ReviewFileChange[] = [];
  private treeRows: ReviewTreeRow[] = [];
  private selectedFileIndex = 0;
  private listScroll = 0;
  private previewScroll = 0;
  private lastPreviewLinesCount = 0;

  // Lightweight inline comment input (replaces complex modal stacks)
  private commenting = false;
  private commentDraft = "";
  private readonly comments = new Map<string, string>();
  private readonly filePreviewCache = new Map<string, FilePreviewResult>();
  private readonly previewLinesCache = new Map<string, string[]>();
  private previewTargetKey = "";

  // Layout geometry for mouse hit-testing
  private dividerX = 42;
  private bodyStartY = 3;
  private bodyHeight = 16;
  private previewVisibleRows = 12;
  private lastEmittedImageKey = "";
  private pillRects = new Map<string, ClickRegion>();
  private closeBtnRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private leftRowMap: Array<{ y1: number; y2: number; index: number }> = [];

  constructor(
    tui: { requestRender: () => void },
    theme: TuiTheme,
    options: ReviewTuiOptions,
    done: (result: ReviewTuiResult | undefined) => void,
  ) {
    this.tui = tui;
    this.renderCoalescer = new RenderCoalescer(() => this.tui.requestRender());
    this.rootDir = options.rootDir;
    this.launchMode = options.mode;
    this.sessionFiles = options.files;
    this.turns = options.turns ?? [];
    this.theme = theme;
    this.done = done;
    this.nerdIcons = options.nerdIcons ?? detectNerdFontSupport();
    this.loadPreviewFn = options.loadPreview ?? loadFileOrDirPreview;
    this.onImageOverlay = options.onImageOverlay;
    this.imageCap = detectTerminalImageProtocol(options.env);
    this.getRows = options.getRows ?? (() => 28);

    this.pickingTurn = this.launchMode === "message" && this.turns.length > 0;
    if (this.turns.length > 0) {
      this.selectedTurnIndex = this.turns.length - 1;
    }
    this.syncActiveFiles();
  }

  isPickingTurn(): boolean {
    return this.pickingTurn;
  }

  getListMode(): ReviewListMode {
    return this.listMode;
  }

  getPreviewMode(): ReviewPreviewMode {
    return this.previewMode;
  }

  getSelectedFile(): ReviewFileChange | undefined {
    return this.activeFiles[this.selectedFileIndex];
  }

  getCommentCount(): number {
    return this.comments.size;
  }

  invalidate(): void {
    // Renders directly from live state.
  }

  private bold(text: string): string {
    return this.theme.bold ? this.theme.bold(text) : `\x1b[1m${text}\x1b[0m`;
  }

  private fg(color: string, text: string): string {
    return this.theme.fg(color, text);
  }

  private styleIcon(name: string, isDir: boolean): string {
    const spec = getFileIcon(name, { isDir, expanded: isDir, nerdIcons: this.nerdIcons });
    return spec.color === "default" ? spec.glyph : this.fg(spec.color, spec.glyph);
  }

  private syncActiveFiles(): void {
    if (this.launchMode === "message" && this.turns.length > 0) {
      this.selectedTurnIndex = Math.max(0, Math.min(this.turns.length - 1, this.selectedTurnIndex));
      this.activeFiles = this.turns[this.selectedTurnIndex]?.files ?? [];
    } else {
      this.activeFiles = this.sessionFiles;
    }
    this.treeRows = buildReviewFileTree(this.activeFiles);
    this.selectedFileIndex = Math.max(0, Math.min(this.activeFiles.length - 1, this.selectedFileIndex));
    this.listScroll = 0;
    this.previewScroll = 0;
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

  private finish(result: ReviewTuiResult | undefined): void {
    this.renderCoalescer.cancel();
    this.syncImageOverlay(undefined);
    this.done(result);
  }

  private submitFeedback(): void {
    const list: ReviewComment[] = [...this.comments.entries()].map(([relPath, note]) => ({
      relPath,
      note,
    }));
    if (list.length === 0) {
      this.finish(undefined);
      return;
    }
    this.finish({
      comments: list,
      formattedFeedback: formatReviewFeedback(list),
    });
  }

  private moveSelection(delta: number): void {
    if (this.pickingTurn) {
      if (this.turns.length === 0) return;
      this.selectedTurnIndex = Math.max(
        0,
        Math.min(this.turns.length - 1, this.selectedTurnIndex + delta),
      );
      this.syncActiveFiles();
      return;
    }
    if (this.activeFiles.length === 0) return;
    const prev = this.selectedFileIndex;
    this.selectedFileIndex = Math.max(
      0,
      Math.min(this.activeFiles.length - 1, this.selectedFileIndex + delta),
    );
    if (this.selectedFileIndex !== prev) {
      this.previewScroll = 0;
    }
  }

  private scrollPreview(delta: number): void {
    const maxScroll = Math.max(0, this.lastPreviewLinesCount - this.previewVisibleRows);
    this.previewScroll = Math.max(0, Math.min(maxScroll, this.previewScroll + delta));
  }

  private startComment(): void {
    const file = this.getSelectedFile();
    if (!file) return;
    this.commenting = true;
    this.commentDraft = this.comments.get(file.relPath) ?? "";
  }

  private commitComment(): void {
    const file = this.getSelectedFile();
    if (file) {
      const trimmed = this.commentDraft.trim();
      if (trimmed) this.comments.set(file.relPath, trimmed);
      else this.comments.delete(file.relPath);
    }
    this.commenting = false;
    this.commentDraft = "";
  }

  handleInput(data: string): void {
    if (this.commenting) {
      if (matchesKey(data, "escape")) {
        this.commenting = false;
        this.commentDraft = "";
        return;
      }
      if (matchesKey(data, "enter") || matchesKey(data, "return")) {
        this.commitComment();
        return;
      }
      if (matchesKey(data, "backspace")) {
        this.commentDraft = Array.from(this.commentDraft).slice(0, -1).join("");
        return;
      }
      let printable = data;
      if (printable.startsWith("\x1b[200~") && printable.endsWith("\x1b[201~")) {
        printable = printable.slice(6, -6).replace(/[\r\n]+/g, " ");
      }
      if (!printable.startsWith("\x1b") && !/[\x00-\x1f\x7f]/.test(printable)) {
        this.commentDraft += printable;
      }
      return;
    }

    if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
      if (this.launchMode === "message" && !this.pickingTurn && this.turns.length > 0) {
        this.pickingTurn = true;
        this.syncImageOverlay(undefined);
        return;
      }
      this.finish(undefined);
      return;
    }
    if (matchesKey(data, "ctrl+s")) {
      this.submitFeedback();
      return;
    }
    if (matchesKey(data, "up") || matchesKey(data, "ctrl+p") || data === "k") {
      this.moveSelection(-1);
      return;
    }
    if (matchesKey(data, "down") || matchesKey(data, "ctrl+n") || data === "j") {
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
    if (matchesKey(data, "tab")) {
      if (this.pickingTurn) {
        this.pickingTurn = false;
      } else {
        this.previewMode = this.previewMode === "diff" ? "file" : "diff";
        this.previewScroll = 0;
      }
      return;
    }
    if (data === "t" && !this.pickingTurn) {
      this.listMode = this.listMode === "tree" ? "list" : "tree";
      return;
    }
    if (matchesKey(data, "enter") || matchesKey(data, "return")) {
      if (this.pickingTurn) {
        this.pickingTurn = false;
        this.selectedFileIndex = 0;
        this.previewScroll = 0;
        return;
      }
      this.startComment();
      return;
    }
    if (data === "c" && !this.pickingTurn) {
      this.startComment();
    }
  }

  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    const { x, y, type } = event;

    if (type === "wheel") {
      const delta = (event.wheelDelta ?? 0) > 0 ? 1 : -1;
      if (y >= this.bodyStartY && y < this.bodyStartY + this.bodyHeight) {
        if (x < this.dividerX) {
          this.moveSelection(delta);
          return { handled: true, render: true };
        }
        this.scrollPreview(delta * 3);
        this.renderCoalescer.schedule();
        return { handled: true, render: false };
      }
      return { handled: true, render: false };
    }

    if (type === "press" && event.button === "left") {
      if (hitRegion(this.closeBtnRect, x, y)) {
        this.finish(undefined);
        return { handled: true, render: false };
      }
      if (hitRegion(this.pillRects.get("send"), x, y) && this.comments.size > 0) {
        this.submitFeedback();
        return { handled: true, render: false };
      }
      if (hitRegion(this.pillRects.get("turn"), x, y) && this.launchMode === "message" && this.turns.length > 0) {
        this.pickingTurn = !this.pickingTurn;
        return { handled: true, render: true };
      }
      if (hitRegion(this.pillRects.get("list"), x, y) && !this.pickingTurn) {
        this.listMode = this.listMode === "tree" ? "list" : "tree";
        return { handled: true, render: true };
      }
      if (hitRegion(this.pillRects.get("preview"), x, y) && !this.pickingTurn) {
        this.previewMode = this.previewMode === "diff" ? "file" : "diff";
        this.previewScroll = 0;
        return { handled: true, render: true };
      }
      if (x >= 1 && x < this.dividerX) {
        for (const row of this.leftRowMap) {
          if (y >= row.y1 && y <= row.y2) {
            if (this.pickingTurn) {
              this.selectedTurnIndex = row.index;
              this.syncActiveFiles();
            } else {
              this.selectedFileIndex = row.index;
              this.previewScroll = 0;
            }
            return { handled: true, render: true };
          }
        }
      }
      return { handled: true, render: false };
    }

    if (type === "click" && event.button === "left" && (event.clickCount ?? 1) >= 2) {
      if (x >= 1 && x < this.dividerX) {
        for (const row of this.leftRowMap) {
          if (y >= row.y1 && y <= row.y2) {
            if (this.pickingTurn) {
              this.selectedTurnIndex = row.index;
              this.syncActiveFiles();
              this.pickingTurn = false;
            } else {
              this.selectedFileIndex = row.index;
              this.startComment();
            }
            return { handled: true, render: true };
          }
        }
      }
    }

    return { handled: true, render: false };
  }

  private renderLeftPane(innerW: number, height: number): string[] {
    this.leftRowMap = [];

    if (this.pickingTurn) {
      if (this.turns.length === 0) {
        const empty = ["", this.fg("dim", " No edited messages to review")];
        while (empty.length < height) empty.push("");
        return empty.slice(0, height).map((l) => fitWidth(l, innerW));
      }
      const lines: string[] = [];
      const page = Math.max(1, Math.floor(height / 2));
      if (this.selectedTurnIndex < this.listScroll) this.listScroll = this.selectedTurnIndex;
      if (this.selectedTurnIndex >= this.listScroll + page) {
        this.listScroll = this.selectedTurnIndex - page + 1;
      }
      for (let slot = 0; slot < page; slot++) {
        const idx = this.listScroll + slot;
        if (idx >= this.turns.length) break;
        const turn = this.turns[idx]!;
        const isSelected = idx === this.selectedTurnIndex;
        const startY = this.bodyStartY + lines.length;
        const marker = isSelected ? this.fg("accent", "▸ ") : "  ";
        const badge = this.fg("dim", ` (${turn.files.length}f)`);
        const promptBudget = Math.max(6, innerW - 2 - visibleWidth(badge));
        const label = truncateToWidth(`#${turn.turnIndex} ${turn.prompt}`, promptBudget, "…");
        const styled = isSelected ? this.bold(this.fg("accent", label)) : label;
        lines.push(fitWidth(`${marker}${styled}${badge}`, innerW));
        if (lines.length < height) {
          const summary = turn.files.map((f) => f.name).slice(0, 3).join(", ");
          lines.push(fitWidth(`  ${this.fg("dim", summary)}`, innerW));
        }
        this.leftRowMap.push({ y1: startY, y2: this.bodyStartY + lines.length - 1, index: idx });
      }
      while (lines.length < height) lines.push(" ".repeat(innerW));
      return lines.slice(0, height);
    }

    if (this.activeFiles.length === 0) {
      const empty = ["", this.fg("dim", " No file changes to review")];
      while (empty.length < height) empty.push("");
      return empty.slice(0, height).map((l) => fitWidth(l, innerW));
    }

    const lines: string[] = [];
    if (this.listMode === "tree") {
      const selectedRowIdx = this.treeRows.findIndex(
        (r) => r.kind === "file" && r.fileIndex === this.selectedFileIndex,
      );
      if (selectedRowIdx >= 0) {
        if (selectedRowIdx < this.listScroll) this.listScroll = selectedRowIdx;
        if (selectedRowIdx >= this.listScroll + height) {
          this.listScroll = selectedRowIdx - height + 1;
        }
      }
      for (let slot = 0; slot < height; slot++) {
        const rIdx = this.listScroll + slot;
        if (rIdx >= this.treeRows.length) break;
        const row = this.treeRows[rIdx]!;
        const y = this.bodyStartY + lines.length;
        let indent = "";
        for (const hasNext of row.ancestorHasNext) {
          indent += hasNext ? this.fg("border", "│ ") : "  ";
        }
        if (row.depth > 0) indent += this.fg("border", row.isLast ? "└─" : "├─");

        if (row.kind === "dir") {
          const icon = this.styleIcon(row.name, true);
          lines.push(fitWidth(` ${indent}${icon} ${this.fg("dim", `${row.name}/`)}`, innerW));
          continue;
        }
        const fIdx = row.fileIndex!;
        const file = this.activeFiles[fIdx]!;
        const isSelected = fIdx === this.selectedFileIndex;
        const marker = isSelected ? this.fg("accent", "▸") : " ";
        const icon = this.styleIcon(file.name, false);
        const noteMark = this.comments.has(file.relPath) ? this.fg("warning", "●") : "";
        const stats = `${this.fg("success", `+${file.additions}`)} ${this.fg("error", `-${file.deletions}`)}`;
        const nameStyled = isSelected ? this.bold(this.fg("accent", file.name)) : file.name;
        const left = `${marker}${indent}${icon} ${nameStyled}${noteMark ? ` ${noteMark}` : ""}`;
        const gap = Math.max(1, innerW - visibleWidth(left) - visibleWidth(stats) - 1);
        lines.push(fitWidth(`${left}${" ".repeat(gap)}${stats} `, innerW));
        this.leftRowMap.push({ y1: y, y2: y, index: fIdx });
      }
    } else {
      if (this.selectedFileIndex < this.listScroll) this.listScroll = this.selectedFileIndex;
      if (this.selectedFileIndex >= this.listScroll + height) {
        this.listScroll = this.selectedFileIndex - height + 1;
      }
      for (let slot = 0; slot < height; slot++) {
        const fIdx = this.listScroll + slot;
        if (fIdx >= this.activeFiles.length) break;
        const file = this.activeFiles[fIdx]!;
        const isSelected = fIdx === this.selectedFileIndex;
        const y = this.bodyStartY + lines.length;
        const marker = isSelected ? this.fg("accent", "▸ ") : "  ";
        const icon = this.styleIcon(file.name, false);
        const noteMark = this.comments.has(file.relPath) ? this.fg("warning", " ●") : "";
        const stats = `${this.fg("success", `+${file.additions}`)} ${this.fg("error", `-${file.deletions}`)}`;
        const labelBudget = Math.max(6, innerW - 5 - visibleWidth(stats) - visibleWidth(noteMark));
        const pathText = truncateToWidth(file.relPath, labelBudget, "…");
        const styled = isSelected ? this.bold(this.fg("accent", pathText)) : pathText;
        const left = `${marker}${icon} ${styled}${noteMark}`;
        const gap = Math.max(1, innerW - visibleWidth(left) - visibleWidth(stats) - 1);
        lines.push(fitWidth(`${left}${" ".repeat(gap)}${stats} `, innerW));
        this.leftRowMap.push({ y1: y, y2: y, index: fIdx });
      }
    }

    while (lines.length < height) lines.push(" ".repeat(innerW));
    return lines.slice(0, height);
  }

  private renderRightPane(innerW: number, height: number): string[] {
    const file = this.getSelectedFile();
    if (!file) {
      this.syncImageOverlay(undefined);
      const empty = ["", this.fg("dim", "  Select a file to inspect changes.")];
      while (empty.length < height) empty.push("");
      return empty.slice(0, height).map((l) => fitWidth(l, innerW));
    }

    const targetKey = `${this.previewMode}:${file.relPath}`;
    if (targetKey !== this.previewTargetKey) {
      this.previewLinesCache.clear();
      this.previewTargetKey = targetKey;
    }

    const icon = this.styleIcon(file.name, false);
    const savedNote = this.comments.get(file.relPath);
    const metaLines: string[] = [
      ` ${icon} ${this.bold(this.fg("accent", file.relPath))}  ${this.fg("success", `+${file.additions}`)} ${this.fg("error", `-${file.deletions}`)}  ${this.fg("dim", `${file.op} ×${file.opsCount}`)}`,
    ];
    if (savedNote) {
      metaLines.push(` ${this.fg("warning", "Note:")} ${savedNote}`);
    }
    const divider = this.fg("border", "─".repeat(innerW));
    const headerBlock = [...metaLines, divider];
    const previewHeight = Math.max(1, height - headerBlock.length);
    this.previewVisibleRows = previewHeight;

    // Direct HD / half-block preview for modified or added image files.
    if (isImageFilePath(file.relPath) && !this.pickingTurn) {
      let prev = this.filePreviewCache.get(file.relPath);
      if (!prev) {
        prev = this.loadPreviewFn(this.rootDir, file.relPath, {
          showHidden: true,
          nerdIcons: this.nerdIcons,
        });
        this.filePreviewCache.set(file.relPath, prev);
      }
      if (prev.kind === "image" && prev.absPath) {
        const useNativeHd = Boolean(this.onImageOverlay && this.imageCap.protocol !== "halfblock");
        if (useNativeHd && innerW >= 6 && previewHeight >= 3) {
          this.syncImageOverlay({
            path: prev.absPath,
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
          prev.absPath,
          Math.max(4, innerW - 2),
          Math.max(2, previewHeight),
          prev.imageMeta,
        );
        const imgLines = halfBlock
          ? halfBlock.map((row) => ` ${row}`)
          : prev.lines.map((l) => ` ${this.fg("dim", l)}`);
        this.lastPreviewLinesCount = imgLines.length;
        while (imgLines.length < previewHeight) imgLines.push("");
        return [...headerBlock, ...imgLines]
          .slice(0, height)
          .map((line) => fitWidth(line, innerW));
      }
    }

    this.syncImageOverlay(undefined);

    const cacheKey = `${targetKey}:${innerW}`;
    let rendered = this.previewLinesCache.get(cacheKey);
    if (!rendered) {
      rendered = [];
      if (this.previewMode === "diff") {
        for (const dl of file.diffLines) {
          const clean = dl.text.replace(/\t/g, "  ");
          if (dl.kind === "hunk") {
            rendered.push(` ${this.fg("accent", clean)}`);
          } else if (dl.kind === "add") {
            const num = dl.newLine ? String(dl.newLine).padStart(4, " ") : "    ";
            rendered.push(` ${this.fg("dim", num)} ${this.fg("success", `+ ${clean}`)}`);
          } else if (dl.kind === "del") {
            const num = dl.oldLine ? String(dl.oldLine).padStart(4, " ") : "    ";
            rendered.push(` ${this.fg("dim", num)} ${this.fg("error", `- ${clean}`)}`);
          } else {
            const num = dl.newLine ? String(dl.newLine).padStart(4, " ") : "    ";
            rendered.push(` ${this.fg("dim", num)}   ${clean}`);
          }
        }
      } else {
        let prev = this.filePreviewCache.get(file.relPath);
        if (!prev) {
          prev = this.loadPreviewFn(this.rootDir, file.relPath, {
            showHidden: true,
            nerdIcons: this.nerdIcons,
          });
          this.filePreviewCache.set(file.relPath, prev);
        }
        const gutterW = Math.max(3, String(prev.lines.length).length);
        for (let i = 0; i < prev.lines.length; i++) {
          const num = String(i + 1).padStart(gutterW, " ");
          rendered.push(` ${this.fg("dim", num)} ${this.fg("border", "│")} ${prev.lines[i]!.replace(/\t/g, "  ")}`);
        }
      }
      this.previewLinesCache.set(cacheKey, rendered);
    }

    this.lastPreviewLinesCount = rendered.length;
    const maxScroll = Math.max(0, rendered.length - previewHeight);
    if (this.previewScroll > maxScroll) this.previewScroll = maxScroll;

    const visible = rendered.slice(this.previewScroll, this.previewScroll + previewHeight);
    while (visible.length < previewHeight) visible.push("");

    return [...headerBlock, ...visible]
      .slice(0, height)
      .map((line) => fitWidth(line, innerW));
  }

  render(width: number): string[] {
    const termRows = this.getRows();
    const targetRows = Math.max(16, Math.floor(termRows * 0.9));
    const geometry = computeSplitGeometry({
      width,
      rows: termRows,
      leftRatio: 0.38,
      hasSearchBar: false,
      minWidth: 60,
      minLeftWidth: 24,
      minRightWidth: 26,
      heightRatio: 0.9,
      maxRows: targetRows,
    });
    this.dividerX = geometry.dividerX;
    this.bodyStartY = geometry.bodyStartY;
    this.bodyHeight = geometry.bodyHeight;

    const title =
      this.launchMode === "message"
        ? ` Review Message (${this.turns.length} turns)`
        : ` Review Session (${this.activeFiles.length} files)`;
    const pills = [
      ...(this.launchMode === "message"
        ? [{ id: "turn", label: this.pickingTurn ? "[Files]" : "[Turns]", active: false, color: "accent" }]
        : []),
      { id: "list", label: this.listMode === "tree" ? "[Tree]" : "[List]", active: false, color: "accent" },
      { id: "preview", label: this.previewMode === "diff" ? "[Diff]" : "[File]", active: false, color: "accent" },
      ...(this.comments.size > 0
        ? [{ id: "send", label: `[Send ${this.comments.size}]`, active: true, color: "success" }]
        : []),
    ];

    const frame = renderSplitOverlayFrame({
      geometry,
      titlePrefix: `${this.bold(title)}  `,
      pills,
      leftLines: this.renderLeftPane(geometry.leftWidth, geometry.bodyHeight),
      rightLines: this.renderRightPane(geometry.rightWidth, geometry.bodyHeight),
      footerCustom: this.commenting
        ? ` ${this.fg("warning", "Note:")} ${this.commentDraft}\x1b[7m \x1b[27m  ${this.fg("dim", "(Enter:save  Esc:cancel)")}`
        : undefined,
      footerHints: [
        { key: "↑↓/Wheel", label: "nav" },
        { key: "Tab", label: "Diff/File" },
        { key: "t", label: "Tree/List" },
        { key: "Enter/c", label: "note" },
        { key: "Ctrl+S", label: "send" },
        { key: "Esc", label: "close" },
      ],
      theme: this.theme,
    });
    this.pillRects = frame.pillRects;
    this.closeBtnRect = frame.closeRect;
    return frame.lines;
  }
}

export function createReviewTui(
  tui: { requestRender: () => void },
  theme: TuiTheme,
  options: ReviewTuiOptions,
  done: (result: ReviewTuiResult | undefined) => void,
): ReviewTui {
  return new ReviewTui(tui, theme, options, done);
}
