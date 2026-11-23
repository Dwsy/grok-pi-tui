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
import {
  computeSplitGeometry,
  fitWidth,
  highlightTerms,
  hitRegion,
  renderSplitOverlayFrame,
  wrapTextToWidth,
  type ClickRegion,
  type TuiTheme,
} from "../pi-grok-tui-kit/index.ts";
import {
  filterSessionRecords,
  formatRelativeAge,
  formatTimestamp,
  formatTokensM,
  loadSessionPreviewMessages,
  type PreviewMessage,
  type SessionRecord,
  type SessionScope,
  type SessionSearchHit,
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
  /** Optional custom preview loader (injected in tests). */
  loadPreview?: (sessionPath: string) => PreviewMessage[];
  /** Terminal height provider for responsive body sizing. */
  getRows?: () => number;
}

export class SessionPickerTui implements Component {
  private readonly records: readonly SessionRecord[];
  private readonly currentCwd: string;
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly titleBadge: string;
  private readonly confirmLabel: string;
  private readonly done: (selected: SessionRecord | undefined) => void;
  private readonly loadPreview: (sessionPath: string) => PreviewMessage[];
  private readonly getRows: () => number;

  private scope: SessionScope;
  private query: string;
  private hits: SessionSearchHit[] = [];
  private selectedIndex = 0;
  private listScroll = 0;
  private previewScroll = 0;
  private readonly previewCache = new Map<string, PreviewMessage[]>();
  private lastPreviewLinesCount = 0;

  // Layout geometry captured on each render for accurate mouse hit-testing.
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
    this.done = done;
    this.loadPreview = options.loadPreview ?? ((p) => loadSessionPreviewMessages(p));
    this.getRows = options.getRows ?? (() => 28);

    this.globalCount = this.records.length;
    this.projectCount = filterSessionRecords(this.records, {
      scope: "project",
      currentCwd: this.currentCwd,
      query: "",
    }).length;

    this.recomputeHits();
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

  private recomputeHits(): void {
    const prevId = this.hits[this.selectedIndex]?.record.id;
    this.hits = filterSessionRecords(this.records, {
      scope: this.scope,
      currentCwd: this.currentCwd,
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

  private setScope(next: SessionScope): void {
    if (this.scope === next) return;
    this.scope = next;
    this.recomputeHits();
  }

  private toggleScope(): void {
    this.setScope(this.scope === "global" ? "project" : "global");
  }

  private itemsPerPage(): number {
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

  /** Required by Pi TUI Component interface. */
  invalidate(): void {
    // Re-renders directly from live state on every frame.
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

    // Handle bracketed paste (`\x1b[200~...\x1b[201~`) or printable characters.
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
      // Header bar clicks (scope pills & close button).
      if (hitRegion(this.closeBtnRect, x, y)) {
        this.done(undefined);
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

  private renderRightPane(innerW: number, height: number): string[] {
    const selected = this.hits[this.selectedIndex]?.record;
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
      pad(
        `${this.fg("dim", "Project:")} ${this.bold(selected.project)}  ${this.fg("dim", "ID:")} ${selected.id}`,
      ),
      pad(
        `${this.fg("dim", "Created:")} ${formatTimestamp(selected.created)}  ${this.fg("dim", "Updated:")} ${formatTimestamp(selected.modified)}`,
      ),
      pad(
        `${this.fg("dim", "Msgs:")} ${msgsDetail}  ${this.fg("dim", "Tokens:")} ${this.fg("warning", formatTokensM(selected.totalTokens))}  ${this.fg("dim", "Model:")} ${this.fg("success", selected.model)}`,
      ),
    ];

    const divider = this.fg("border", "─".repeat(innerW));
    const headerBlock = [...metaLines, divider];
    const previewHeight = Math.max(1, height - headerBlock.length);
    this.previewVisibleRows = previewHeight;

    const messages = this.getPreviewMessages(selected);
    const allPreviewLines: string[] = [];
    const textWidth = Math.max(8, innerW - 2);

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
        const wrapped = wrapTextToWidth(msg.content, textWidth);
        for (const wline of wrapped) {
          const rendered = this.query.trim()
            ? highlightTerms(wline, this.query, (m) => `\x1b[7m${m}\x1b[27m`)
            : wline;
          allPreviewLines.push(`  ${rendered}`);
        }
        allPreviewLines.push("");
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
      const titleText = truncateToWidth(rec.title, titleBudget, "…");
      const titleStyled = isSelected ? this.bold(this.fg("accent", titleText)) : titleText;
      const gap = Math.max(0, innerW - 2 - visibleWidth(titleText) - visibleWidth(rightAge));
      const row1 = `${marker}${titleStyled}${" ".repeat(gap)}${this.fg("dim", rightAge)}`;

      const metaSummary = hit.matchSnippet
        ? `${rec.project} · ${hit.matchSnippet}`
        : `${rec.project} · ${rec.shortId} · ${rec.messageCount}m · ${formatTokensM(rec.totalTokens)} · ${rec.model}`;
      const row2 = `  ${this.fg("dim", truncateToWidth(metaSummary, Math.max(4, innerW - 2), "…"))}`;

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

  render(width: number): string[] {
    const geometry = computeSplitGeometry({
      width,
      rows: this.getRows(),
      leftRatio: 0.4,
      hasSearchBar: true,
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
        rightLabel: `${this.hits.length}/${this.scope === "global" ? this.globalCount : this.projectCount} · User & Agent only `,
      },
      leftLines: this.renderLeftPane(geometry.leftWidth, geometry.bodyHeight),
      rightLines: this.renderRightPane(geometry.rightWidth, geometry.bodyHeight),
      footerHints: [
        { key: "↑↓/Wheel", label: "select" },
        { key: "Tab/Click", label: "Global/Project" },
        { key: "PgUp/PgDn/Wheel", label: "preview" },
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
