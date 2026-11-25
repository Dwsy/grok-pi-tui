/**
 * Full bordered left/right split TUI for the `@skill` mention picker.
 *
 * Mirrors the `@session` picker architecture:
 * - Outer rounded box border (`╭─╮│╰─╯`) with vertical split divider (`┬│┴`)
 * - Header bar with Global (all) / Project scope toggle (default Global) & `[✕]`
 * - Live search bar filtering skill name, description, and path
 * - Left pane: scrollable 2-line skill cards (name, scope badge, description)
 * - Right pane: skill metadata card (Scope, Path, Updated, Size, Bundled resources,
 *   Description) + scrollable `SKILL.md` preview with search highlighting
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
import {
  computeSplitGeometry,
  fitWidth,
  highlightTerms,
  hitRegion,
  renderSplitOverlayFrame,
  RenderCoalescer,
  wrapTextToWidth,
  type ClickRegion,
  type TuiTheme,
} from "../pi-grok-tui-kit/index.ts";
import {
  filterSkillRecords,
  formatFileSize,
  loadSkillPreviewDetail,
  type SkillPreviewDetail,
  type SkillRecord,
  type SkillScope,
  type SkillSearchHit,
} from "./skill-data.ts";

type PreviewLineStyle = "plain" | "heading" | "fence" | "dim";

export interface SkillPickerTuiOptions {
  records: readonly SkillRecord[];
  initialQuery?: string;
  initialScope?: SkillScope;
  /** Optional custom `SKILL.md` preview loader (injected in tests). */
  loadDetail?: (sourcePath: string | undefined) => SkillPreviewDetail | null;
  /** Terminal height provider for responsive body sizing. */
  getRows?: () => number;
}

export class SkillPickerTui implements Component {
  private readonly records: readonly SkillRecord[];
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly done: (selected: SkillRecord | undefined) => void;
  private readonly loadDetail: (sourcePath: string | undefined) => SkillPreviewDetail | null;
  private readonly getRows: () => number;
  private readonly renderCoalescer: RenderCoalescer;

  private scope: SkillScope;
  private query: string;
  private hits: SkillSearchHit[] = [];
  private selectedIndex = 0;
  private listScroll = 0;
  private previewScroll = 0;
  private readonly detailCache = new Map<string, SkillPreviewDetail | null>();
  private readonly wrappedLinesCache = new Map<string, string[]>();
  private readonly wrappedLineStylesCache = new Map<string, PreviewLineStyle[]>();
  private lastWrappedDetailKey = "";
  private lastPreviewLinesCount = 0;

  // Layout geometry captured per frame for accurate mouse hit-testing.
  private dividerX = 41;
  private bodyStartY = 5;
  private bodyHeight = 14;
  private previewVisibleRows = 10;
  private pillRects = new Map<string, ClickRegion>();
  private closeBtnRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private listRowMap: Array<{ y1: number; y2: number; index: number }> = [];

  private readonly globalCount: number;
  private readonly projectCount: number;

  constructor(
    tui: { requestRender: () => void },
    theme: TuiTheme,
    options: SkillPickerTuiOptions,
    done: (selected: SkillRecord | undefined) => void,
  ) {
    this.tui = tui;
    this.renderCoalescer = new RenderCoalescer(() => this.tui.requestRender());
    this.theme = theme;
    this.records = options.records;
    this.scope = options.initialScope ?? "global";
    this.query = options.initialQuery ?? "";
    this.done = done;
    this.loadDetail = options.loadDetail ?? ((p) => loadSkillPreviewDetail(p));
    this.getRows = options.getRows ?? (() => 28);

    this.globalCount = this.records.length;
    this.projectCount = this.records.filter((r) => r.origin === "project").length;

    this.recomputeHits();
  }

  getScope(): SkillScope {
    return this.scope;
  }

  getQuery(): string {
    return this.query;
  }

  getSelectedRecord(): SkillRecord | undefined {
    return this.hits[this.selectedIndex]?.record;
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

  private recomputeHits(): void {
    const prevId = this.hits[this.selectedIndex]?.record.id;
    this.hits = filterSkillRecords(this.records, {
      scope: this.scope,
      query: this.query,
    });
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

  private setScope(next: SkillScope): void {
    if (this.scope === next) return;
    this.scope = next;
    this.recomputeHits();
  }

  private toggleScope(): void {
    this.setScope(this.scope === "global" ? "project" : "global");
  }

  private itemsPerPage(): number {
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

  private getDetail(record: SkillRecord): SkillPreviewDetail | null {
    const key = record.sourcePath ?? record.id;
    if (this.detailCache.has(key)) {
      return this.detailCache.get(key) ?? null;
    }
    const detail = this.loadDetail(record.sourcePath);
    this.detailCache.set(key, detail);
    return detail;
  }

  private finish(selected: SkillRecord | undefined): void {
    this.renderCoalescer.cancel();
    this.done(selected);
  }

  private syncWrappedLineCache(detailKey: string, detail: SkillPreviewDetail | null): void {
    const nextKey = `${detailKey}:${detail?.body ?? ""}`;
    if (nextKey === this.lastWrappedDetailKey) return;
    this.wrappedLinesCache.clear();
    this.wrappedLineStylesCache.clear();
    this.lastWrappedDetailKey = nextKey;
  }

  private getWrappedPreviewLines(
    detail: SkillPreviewDetail | null,
    detailKey: string,
    textWidth: number,
  ): { lines: string[]; styles: PreviewLineStyle[] } {
    const cacheKey = `${detailKey}:${textWidth}`;
    const cachedLines = this.wrappedLinesCache.get(cacheKey);
    const cachedStyles = this.wrappedLineStylesCache.get(cacheKey);
    if (cachedLines && cachedStyles) return { lines: cachedLines, styles: cachedStyles };

    const lines: string[] = [];
    const styles: PreviewLineStyle[] = [];
    if (!detail?.body) {
      lines.push("(SKILL.md content unavailable for preview)");
      styles.push("dim");
    } else {
      for (const rawLine of detail.body.split("\n")) {
        const style: PreviewLineStyle = /^#{1,6}\s+/.test(rawLine)
          ? "heading"
          : rawLine.trim().startsWith("```")
            ? "fence"
            : "plain";
        for (const wline of wrapTextToWidth(rawLine, textWidth)) {
          lines.push(wline);
          styles.push(style);
        }
      }
    }

    this.wrappedLinesCache.set(cacheKey, lines);
    this.wrappedLineStylesCache.set(cacheKey, styles);
    return { lines, styles };
  }

  handleInput(data: string): void {
    if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
      this.finish(undefined);
      return;
    }
    if (matchesKey(data, "enter") || matchesKey(data, "return")) {
      this.finish(this.getSelectedRecord());
      return;
    }
    if (matchesKey(data, "tab") || matchesKey(data, "ctrl+g")) {
      this.toggleScope();
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
    if (matchesKey(data, "ctrl+w") || matchesKey(data, "ctrl+l")) {
      if (this.query.length > 0) {
        this.query = "";
        this.recomputeHits();
      }
      return;
    }
    if (matchesKey(data, "backspace")) {
      if (this.query.length > 0) {
        this.query = Array.from(this.query).slice(0, -1).join("");
        this.recomputeHits();
      }
      return;
    }

    let printable = data;
    if (printable.startsWith("\x1b[200~") && printable.endsWith("\x1b[201~")) {
      printable = printable.slice(6, -6).replace(/[\r\n]+/g, " ");
    }
    if (!printable.startsWith("\x1b") && !/[\x00-\x1f\x7f]/.test(printable)) {
      this.query += printable;
      this.recomputeHits();
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
      if (hitRegion(this.pillRects.get("global"), x, y)) {
        this.setScope("global");
        return { handled: true, render: true };
      }
      if (hitRegion(this.pillRects.get("project"), x, y)) {
        this.setScope("project");
        return { handled: true, render: true };
      }
      if (x >= 1 && x < this.dividerX) {
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
      if (x >= 1 && x < this.dividerX) {
        for (const row of this.listRowMap) {
          if (y >= row.y1 && y <= row.y2) {
            this.selectedIndex = row.index;
            this.finish(this.getSelectedRecord());
            return { handled: true, render: false };
          }
        }
      }
    }

    return { handled: true, render: false };
  }

  private renderLeftPane(innerW: number, height: number): string[] {
    this.listRowMap = [];
    this.clampListScroll();

    if (this.hits.length === 0) {
      const lines = [
        "",
        this.fg("dim", " No matching skills"),
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
      const rec = this.hits[idx]!.record;
      const isSelected = idx === this.selectedIndex;
      const startY = this.bodyStartY + lines.length;

      const marker = isSelected ? this.fg("accent", "▸ ") : "  ";
      const badgeText = rec.origin === "project" ? " [Project]" : " [Global]";
      const badgeStyled =
        rec.origin === "project" ? this.fg("success", badgeText) : this.fg("dim", badgeText);
      const nameBudget = Math.max(6, innerW - 2 - visibleWidth(badgeText));
      const nameText = truncateToWidth(rec.id, nameBudget, "…");
      const nameStyled = isSelected ? this.bold(this.fg("accent", nameText)) : nameText;
      const gap = Math.max(0, innerW - 2 - visibleWidth(nameText) - visibleWidth(badgeText));
      const row1 = `${marker}${nameStyled}${" ".repeat(gap)}${badgeStyled}`;

      const desc = rec.description || rec.displayPath;
      const row2 = `  ${this.fg("dim", truncateToWidth(desc, Math.max(4, innerW - 2), "…"))}`;

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

  private renderRightPane(innerW: number, height: number): string[] {
    const selected = this.hits[this.selectedIndex]?.record;
    if (!selected) {
      const empty = [
        "",
        this.fg("dim", "  No skill selected."),
        this.fg("dim", "  Try clearing search or switching Global / Project."),
      ];
      while (empty.length < height) empty.push("");
      return empty.slice(0, height).map((l) => fitWidth(l, innerW));
    }

    const detail = this.getDetail(selected);
    const pad = (s: string) => ` ${s}`;
    const scopeLabel =
      selected.origin === "project"
        ? this.fg("success", "Project")
        : this.fg("accent", "Global");
    const sizeInfo = detail
      ? `${detail.lineCount}L / ${formatFileSize(detail.sizeBytes)}`
      : "—";
    const updatedInfo = detail?.modified ?? "—";
    const extrasInfo =
      detail && detail.bundledDirs.length > 0 ? `  ${this.fg("dim", "Extras:")} ${detail.bundledDirs.join(", ")}` : "";
    const descText = selected.description || detail?.frontmatter.description || "—";

    const metaLines: string[] = [
      pad(
        `${this.bold(this.fg("accent", selected.title))}  ${this.fg("dim", "Scope:")} ${scopeLabel}${extrasInfo}`,
      ),
      pad(`${this.fg("dim", "Path:")} ${selected.displayPath}`),
      pad(`${this.fg("dim", "Updated:")} ${updatedInfo}  ${this.fg("dim", "Size:")} ${sizeInfo}`),
      pad(`${this.fg("dim", "Desc:")} ${descText}`),
    ];

    const divider = this.fg("border", "─".repeat(innerW));
    const headerBlock = [...metaLines, divider];
    const previewHeight = Math.max(1, height - headerBlock.length);
    this.previewVisibleRows = previewHeight;

    const detailKey = selected.sourcePath ?? selected.id;
    this.syncWrappedLineCache(detailKey, detail);
    const textWidth = Math.max(8, innerW - 2);
    const wrapped = this.getWrappedPreviewLines(detail, detailKey, textWidth);

    this.lastPreviewLinesCount = wrapped.lines.length;
    const maxScroll = Math.max(0, wrapped.lines.length - previewHeight);
    if (this.previewScroll > maxScroll) this.previewScroll = maxScroll;

    const visiblePreview: string[] = [];
    const start = Math.min(this.previewScroll, wrapped.lines.length);
    const end = Math.min(start + previewHeight, wrapped.lines.length);
    for (let i = start; i < end; i++) {
      const wline = wrapped.lines[i] ?? "";
      const style = wrapped.styles[i] ?? "plain";
      let rendered =
        style !== "dim" && this.query.trim()
          ? highlightTerms(wline, this.query, (m) => `\x1b[7m${m}\x1b[27m`)
          : wline;
      if (style === "heading") {
        rendered = this.bold(this.fg("accent", rendered));
      } else if (style === "fence" || style === "dim") {
        rendered = this.fg("dim", rendered);
      }
      visiblePreview.push(` ${rendered}`);
    }
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
      titlePrefix: ` ${this.bold("Skills")} ${this.fg("dim", "(@skill)")}  `,
      pills: [
        { id: "global", label: `[Global ${this.globalCount}]`, active: this.scope === "global" },
        { id: "project", label: `[Project ${this.projectCount}]`, active: this.scope === "project" },
      ],
      search: {
        promptPrefix: ` ${this.fg("accent", "Search:")} `,
        query: this.query,
        placeholder: "type to search skill name, description, or path...",
        rightLabel: `${this.hits.length}/${this.scope === "global" ? this.globalCount : this.projectCount} skills `,
      },
      leftLines: this.renderLeftPane(geometry.leftWidth, geometry.bodyHeight),
      rightLines: this.renderRightPane(geometry.rightWidth, geometry.bodyHeight),
      footerHints: [
        { key: "↑↓/Wheel", label: "select" },
        { key: "Tab/Click", label: "Global/Project" },
        { key: "PgUp/PgDn/Wheel", label: "preview" },
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

export function createSkillPickerTui(
  tui: { requestRender: () => void },
  theme: TuiTheme,
  options: SkillPickerTuiOptions,
  done: (selected: SkillRecord | undefined) => void,
): SkillPickerTui {
  return new SkillPickerTui(tui, theme, options, done);
}
