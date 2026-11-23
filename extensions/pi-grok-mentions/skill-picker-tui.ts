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
import { fitWidth, wrapTextToWidth, type SessionPickerTheme } from "./session-picker-tui.ts";
import {
  filterSkillRecords,
  formatFileSize,
  loadSkillPreviewDetail,
  type SkillPreviewDetail,
  type SkillRecord,
  type SkillScope,
  type SkillSearchHit,
} from "./skill-data.ts";

export interface SkillPickerTuiOptions {
  records: readonly SkillRecord[];
  initialQuery?: string;
  initialScope?: SkillScope;
  /** Optional custom `SKILL.md` preview loader (injected in tests). */
  loadDetail?: (sourcePath: string | undefined) => SkillPreviewDetail | null;
  /** Terminal height provider for responsive body sizing. */
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

export class SkillPickerTui implements Component {
  private readonly records: readonly SkillRecord[];
  private readonly tui: { requestRender: () => void };
  private readonly theme: SessionPickerTheme;
  private readonly done: (selected: SkillRecord | undefined) => void;
  private readonly loadDetail: (sourcePath: string | undefined) => SkillPreviewDetail | null;
  private readonly getRows: () => number;

  private scope: SkillScope;
  private query: string;
  private hits: SkillSearchHit[] = [];
  private selectedIndex = 0;
  private listScroll = 0;
  private previewScroll = 0;
  private readonly detailCache = new Map<string, SkillPreviewDetail | null>();
  private lastPreviewLinesCount = 0;

  // Layout geometry captured per frame for accurate mouse hit-testing.
  private dividerX = 41;
  private bodyStartY = 5;
  private bodyHeight = 14;
  private previewVisibleRows = 10;
  private globalPillRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private projectPillRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private closeBtnRect: ClickRegion = { x1: 0, x2: 0, y: 1 };
  private listRowMap: Array<{ y1: number; y2: number; index: number }> = [];

  private readonly globalCount: number;
  private readonly projectCount: number;

  constructor(
    tui: { requestRender: () => void },
    theme: SessionPickerTheme,
    options: SkillPickerTuiOptions,
    done: (selected: SkillRecord | undefined) => void,
  ) {
    this.tui = tui;
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

  handleInput(data: string): void {
    if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
      this.done(undefined);
      return;
    }
    if (matchesKey(data, "enter") || matchesKey(data, "return")) {
      this.done(this.getSelectedRecord());
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
        } else {
          this.scrollPreview(delta * 3);
        }
        return { handled: true, render: true };
      }
      return { handled: true, render: false };
    }

    if (type === "press" && event.button === "left") {
      if (y === this.closeBtnRect.y && x >= this.closeBtnRect.x1 && x <= this.closeBtnRect.x2) {
        this.done(undefined);
        return { handled: true, render: false };
      }
      if (y === this.globalPillRect.y && x >= this.globalPillRect.x1 && x <= this.globalPillRect.x2) {
        this.setScope("global");
        return { handled: true, render: true };
      }
      if (y === this.projectPillRect.y && x >= this.projectPillRect.x1 && x <= this.projectPillRect.x2) {
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
            this.done(this.getSelectedRecord());
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

    const allPreviewLines: string[] = [];
    const textWidth = Math.max(8, innerW - 2);

    if (!detail || !detail.body) {
      allPreviewLines.push(this.fg("dim", " (SKILL.md content unavailable for preview)"));
    } else {
      for (const rawLine of detail.body.split("\n")) {
        const isHeading = /^#{1,6}\s+/.test(rawLine);
        const isFence = rawLine.trim().startsWith("```");
        const wrapped = wrapTextToWidth(rawLine, textWidth);
        for (const wline of wrapped) {
          let rendered = this.query.trim()
            ? highlightTerms(wline, this.query, (m) => `\x1b[7m${m}\x1b[27m`)
            : wline;
          if (isHeading) {
            rendered = this.bold(this.fg("accent", rendered));
          } else if (isFence) {
            rendered = this.fg("dim", rendered);
          }
          allPreviewLines.push(` ${rendered}`);
        }
      }
    }

    this.lastPreviewLinesCount = allPreviewLines.length;
    const maxScroll = Math.max(0, allPreviewLines.length - previewHeight);
    if (this.previewScroll > maxScroll) this.previewScroll = maxScroll;

    const visiblePreview = allPreviewLines.slice(
      this.previewScroll,
      this.previewScroll + previewHeight,
    );
    while (visiblePreview.length < previewHeight) visiblePreview.push("");

    return [...headerBlock, ...visiblePreview]
      .slice(0, height)
      .map((line) => fitWidth(line, innerW));
  }

  render(width: number): string[] {
    const totalW = Math.max(56, Math.floor(width));
    const innerW = totalW - 2;

    const leftW = Math.max(24, Math.min(innerW - 28, Math.floor(innerW * 0.4)));
    const rightW = Math.max(24, innerW - leftW - 1);
    this.dividerX = 1 + leftW;

    const totalRows = Math.max(16, Math.min(34, Math.floor(this.getRows() * 0.82)));
    const bodyH = Math.max(8, totalRows - 8);
    this.bodyStartY = 5;
    this.bodyHeight = bodyH;

    const b = (s: string) => this.fg("border", s);
    const topBorder = b(`╭${"─".repeat(innerW)}╮`);
    const midRule = b(`├${"─".repeat(innerW)}┤`);
    const splitTop = b(`├${"─".repeat(leftW)}┬${"─".repeat(rightW)}┤`);
    const splitBottom = b(`├${"─".repeat(leftW)}┴${"─".repeat(rightW)}┤`);
    const bottomBorder = b(`╰${"─".repeat(innerW)}╯`);

    // Row 1: Header with title, Global/Project scope pills, and close button.
    const titlePrefix = ` ${this.bold("Skills")} ${this.fg("dim", "(@skill)")}  `;
    const globalText = `[Global ${this.globalCount}]`;
    const projectText = `[Project ${this.projectCount}]`;
    const closeText = "[✕]";

    const globalStyled =
      this.scope === "global"
        ? this.bold(this.fg("accent", globalText))
        : this.fg("dim", globalText);
    const projectStyled =
      this.scope === "project"
        ? this.bold(this.fg("accent", projectText))
        : this.fg("dim", projectText);

    const prefixW = visibleWidth(titlePrefix);
    const gStart = 1 + prefixW;
    const gEnd = gStart + visibleWidth(globalText) - 1;
    const pStart = gEnd + 2;
    const pEnd = pStart + visibleWidth(projectText) - 1;
    this.globalPillRect = { x1: gStart, x2: gEnd, y: 1 };
    this.projectPillRect = { x1: pStart, x2: pEnd, y: 1 };

    const leftHeader = `${titlePrefix}${globalStyled} ${projectStyled}`;
    const closeW = visibleWidth(closeText);
    const headerGap = Math.max(1, innerW - visibleWidth(leftHeader) - closeW - 1);
    const closeStart = 1 + innerW - closeW - 1;
    this.closeBtnRect = { x1: closeStart, x2: closeStart + closeW - 1, y: 1 };
    const headerLine = `${b("│")}${fitWidth(`${leftHeader}${" ".repeat(headerGap)}${this.fg("dim", closeText)} `, innerW)}${b("│")}`;

    // Row 3: Search input + result counter.
    const countLabel = `${this.hits.length}/${this.scope === "global" ? this.globalCount : this.projectCount} skills `;
    const searchPrefix = ` ${this.fg("accent", "Search:")} `;
    const queryDisplay = this.query
      ? `${this.query}\x1b[7m \x1b[27m`
      : `\x1b[7m \x1b[27m${this.fg("dim", "type to search skill name, description, or path...")}`;
    const searchBudget = Math.max(10, innerW - visibleWidth(countLabel) - 1);
    const searchLeft = fitWidth(`${searchPrefix}${queryDisplay}`, searchBudget);
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

    // Footer hints row.
    const footerText =
      ` ${this.fg("accent", "↑↓/Wheel")}${this.fg("dim", ":select  ")}` +
      `${this.fg("accent", "Tab/Click")}${this.fg("dim", ":Global/Project  ")}` +
      `${this.fg("accent", "PgUp/PgDn/Wheel")}${this.fg("dim", ":preview  ")}` +
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

export function createSkillPickerTui(
  tui: { requestRender: () => void },
  theme: SessionPickerTheme,
  options: SkillPickerTuiOptions,
  done: (selected: SkillRecord | undefined) => void,
): SkillPickerTui {
  return new SkillPickerTui(tui, theme, options, done);
}
