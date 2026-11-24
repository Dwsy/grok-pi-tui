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
  /**
   * Render as a full-screen edge-to-edge Codex-style split workspace
   * (used by `/resume`) instead of a floating bordered modal box.
   */
  fullscreen?: boolean;
  /** Optional custom preview loader (injected in tests). */
  loadPreview?: (sessionPath: string) => PreviewMessage[];
  /** Optional full-text database search across all historical sessions. */
  searchRecords?: (options: {
    query: string;
    scope: SessionScope;
    currentCwd: string;
  }) => SessionSearchHit[];
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
 * Curated medium-saturation identity palette modeled on Codex's
 * `thread_color.rs` (high legibility on both light and dark terminals).
 */
const SESSION_IDENTITY_PALETTE: readonly string[] = [
  "\x1b[38;2;37;99;235m",   // cobalt blue
  "\x1b[38;2;13;148;136m",  // teal
  "\x1b[38;2;147;51;234m",  // violet
  "\x1b[38;2;217;119;6m",   // amber
  "\x1b[38;2;225;29;72m",   // rose
  "\x1b[38;2;22;163;74m",   // emerald
  "\x1b[38;2;2;132;199m",   // sky
  "\x1b[38;2;192;38;211m",  // fuchsia
];

/**
 * Deterministic FNV-1a identity color per session ID (matches Codex's
 * `thread_color::color_for_id` so colors stay stable across sorts & filters).
 */
function styleSessionTitle(sessionId: string, text: string): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < sessionId.length; i++) {
    hash ^= sessionId.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  const color = SESSION_IDENTITY_PALETTE[hash % SESSION_IDENTITY_PALETTE.length]!;
  return `${color}${text}\x1b[39m`;
}

function formatCompactDate(raw: string): string {
  const full = formatTimestamp(raw);
  // `YYYY-MM-DD HH:mm` -> `MM-DD HH:mm`
  return full.length >= 16 ? full.slice(5, 16) : full;
}

export class SessionPickerTui implements Component {
  private readonly records: readonly SessionRecord[];
  private readonly currentCwd: string;
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly titleBadge: string;
  private readonly confirmLabel: string;
  private readonly fullscreen: boolean;
  private readonly done: (selected: SessionRecord | undefined) => void;
  private readonly loadPreview: (sessionPath: string) => PreviewMessage[];
  private readonly searchRecords?: (options: {
    query: string;
    scope: SessionScope;
    currentCwd: string;
  }) => SessionSearchHit[];
  private readonly getRows: () => number;

  private scope: SessionScope;
  private query: string;
  private hits: SessionSearchHit[] = [];
  /** Visual row list for fullscreen project-grouped mode. */
  private groupedRows: FullscreenLeftRow[] = [];
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
    this.fullscreen = options.fullscreen === true;
    this.done = done;
    this.loadPreview = options.loadPreview ?? ((p) => loadSessionPreviewMessages(p));
    this.searchRecords = options.searchRecords;
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
    let filtered = filterSessionRecords(this.records, {
      scope: this.scope,
      currentCwd: this.currentCwd,
      query: this.query,
    });
    if (this.query.trim() && this.searchRecords) {
      const dbHits = this.searchRecords({
        query: this.query,
        scope: this.scope,
        currentCwd: this.currentCwd,
      });
      if (dbHits.length > 0) {
        const seen = new Set(filtered.map((h) => h.record.id));
        const merged = [...filtered];
        for (const hit of dbHits) {
          if (!seen.has(hit.record.id)) {
            seen.add(hit.record.id);
            merged.push(hit);
          }
        }
        filtered = merged;
      }
    }
    if (this.fullscreen) {
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
          rows.push({ kind: "item", hitIndex, hit: item });
        }
        groupIdx += 1;
      }
      this.hits = ordered;
      this.groupedRows = rows;
    } else {
      this.hits = filtered;
      this.groupedRows = [];
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

  private setScope(next: SessionScope): void {
    if (this.scope === next) return;
    this.scope = next;
    this.recomputeHits();
  }

  private toggleScope(): void {
    this.setScope(this.scope === "global" ? "project" : "global");
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
      const selectedRowIdx = this.groupedRows.findIndex(
        (r) => r.kind === "item" && r.hitIndex === this.selectedIndex,
      );
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
    if (matchesKey(data, "tab") || matchesKey(data, "shift+tab") || matchesKey(data, "ctrl+g")) {
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

  private renderFullscreenLeftPane(innerW: number, height: number): string[] {
    this.listRowMap = [];
    this.clampListScroll();

    const ageColW = 7;
    const createdColW = innerW >= 88 ? 11 : 0;
    const msgsColW = innerW >= 68 ? 5 : 0;
    const modelColW = innerW >= 54 ? 18 : 0;
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
        modelColW > 0 ? truncateToWidth(shortModelName(rec.model), modelColW, "…") : "";

      if (isSelected) {
        const prefix = " ▸ ○ ";
        const titleBudget = Math.max(8, titleColW - visibleWidth(prefix));
        const titleText = truncateToWidth(rec.title, titleBudget, "…");
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
        const titleBudget = Math.max(8, titleColW - visibleWidth(prefix));
        const titleRaw = truncateToWidth(rec.title, titleBudget, "…");
        const coloredTitle = styleSessionTitle(rec.id, titleRaw);
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
    const selected = this.hits[this.selectedIndex]?.record;
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

    const headerLines: string[] = [pad(this.fg("dim", "Session details")), ""];
    const wrappedTitle = wrapTextToWidth(selected.title, textW).slice(0, 2);
    for (const tLine of wrappedTitle) {
      headerLines.push(pad(this.bold(styleSessionTitle(selected.id, tLine))));
    }
    headerLines.push(
      pad(
        `${this.fg("dim", "○")} ${this.fg("dim", selected.shortId)} · ${msgsDetail} msgs · ${this.fg("warning", formatTokensM(selected.totalTokens))}`,
      ),
    );
    headerLines.push("");
    headerLines.push(
      pad(`${this.fg("dim", "Model:")} ${this.fg("success", shortModelName(selected.model))}`),
    );
    headerLines.push(pad(`${this.fg("dim", "Messages:")} ${msgsDetail}`));
    headerLines.push(
      pad(`${this.fg("dim", "Created:")} ${formatTimestamp(selected.created)}`),
    );
    headerLines.push(
      pad(`${this.fg("dim", "Updated:")} ${formatTimestamp(selected.modified)}`),
    );
    headerLines.push("");
    headerLines.push(pad(this.fg("dim", "Project")));
    for (const pLine of wrapTextToWidth(selected.cwd || selected.project, textW).slice(0, 2)) {
      headerLines.push(pad(pLine));
    }
    headerLines.push("");
    if (selectedHit?.matchSnippet && this.query.trim()) {
      headerLines.push(pad(this.fg("dim", "Match")));
      const highlightedSnippet = highlightTerms(
        truncateToWidth(selectedHit.matchSnippet, textW, "…"),
        this.query,
        (m) => `\x1b[7m${m}\x1b[27m`,
      );
      headerLines.push(pad(highlightedSnippet));
      headerLines.push("");
    }
    headerLines.push(pad(this.fg("dim", "Last messages")));

    const previewHeight = Math.max(1, height - headerLines.length);
    this.previewVisibleRows = previewHeight;

    const messages = this.getPreviewMessages(selected);
    const allPreviewLines: string[] = [];
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
        for (const wline of wrapTextToWidth(msg.content, textW)) {
          const rendered = this.query.trim()
            ? highlightTerms(wline, this.query, (m) => `\x1b[7m${m}\x1b[27m`)
            : wline;
          allPreviewLines.push(pad(rendered));
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

    return [...headerLines, ...visiblePreview]
      .slice(0, height)
      .map((line) => fitWidth(line, innerW));
  }

  private renderFullscreen(width: number): string[] {
    const totalWidth = Math.max(40, width);
    const totalRows = Math.max(12, this.getRows());
    // Reserve 3 top rows (title, tabs+search, divider) and 1 bottom footer row.
    const bodyHeight = Math.max(4, totalRows - 4);
    const minRight = totalWidth >= 80 ? 28 : 20;
    const leftWidth = Math.max(
      20,
      Math.min(totalWidth - minRight - 1, Math.floor((totalWidth - 1) * 0.64)),
    );
    const rightWidth = Math.max(10, totalWidth - 1 - leftWidth);

    this.dividerX = leftWidth;
    this.bodyStartY = 3;
    this.bodyHeight = bodyHeight;

    // Row 0: Codex-style top title bar + clickable close button.
    const titleLeft = `  ${this.bold("Sessions")}  ${this.fg("dim", "Group: Project")}`;
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
    const searchBlock = this.query
      ? `   ${this.fg("accent", "/")} ${this.query}${this.fg("accent", "▏")}`
      : `   ${this.fg("dim", "/ type to filter...")}`;
    const row1Left = `  ${allTab}   ${projTab}${searchBlock}`;
    const row1Right = this.fg("dim", "tab/⇧tab filter  esc close  ");
    const row1Gap = Math.max(1, totalWidth - visibleWidth(row1Left) - visibleWidth(row1Right));
    const row1 = fitWidth(`${row1Left}${" ".repeat(row1Gap)}${row1Right}`, totalWidth);

    // Row 2: Full-width horizontal hairline divider.
    const row2 = this.fg("border", "─".repeat(totalWidth));

    // Body rows: left grouped list | vertical divider | right session details.
    const leftLines = this.renderFullscreenLeftPane(leftWidth, bodyHeight);
    const rightLines = this.renderFullscreenRightPane(rightWidth, bodyHeight);
    const vBar = this.fg("border", "│");
    const bodyLines: string[] = [];
    for (let i = 0; i < bodyHeight; i++) {
      bodyLines.push(`${leftLines[i] ?? " ".repeat(leftWidth)}${vBar}${rightLines[i] ?? " ".repeat(rightWidth)}`);
    }

    // Bottom footer row.
    const totalInScope = this.scope === "global" ? this.globalCount : this.projectCount;
    const footerLeft = this.fg("dim", `  grok-pi · /resume · ${this.hits.length}/${totalInScope}`);
    const footerRight = this.fg(
      "dim",
      `↑↓ select · Tab filter · PgUp/PgDn preview · Enter ${this.confirmLabel} · Esc close  `,
    );
    const footerGap = Math.max(1, totalWidth - visibleWidth(footerLeft) - visibleWidth(footerRight));
    const footer = fitWidth(`${footerLeft}${" ".repeat(footerGap)}${footerRight}`, totalWidth);

    return [row0, row1, row2, ...bodyLines, footer];
  }

  render(width: number): string[] {
    if (this.fullscreen) {
      return this.renderFullscreen(width);
    }
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
