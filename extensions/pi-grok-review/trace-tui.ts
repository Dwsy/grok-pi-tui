/**
 * Bordered two-pane TS TUI overlay for `/trace` and Left-arrow tool trace
 * inspection (`__pi_trace`).
 *
 * Built on `../pi-grok-tui-kit/` with 90vh sizing, clickable kind-filter pills,
 * live multi-term search, Pretty / Raw JSON view toggle, coalesced right-pane
 * wheel scrolling, bounded preview caching, and in-memory divider dragging.
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
  DividerDragController,
  fitWidth,
  highlightTerms,
  hitRegion,
  ListController,
  RenderCoalescer,
  renderSplitOverlayFrame,
  styleBold,
  wrapTextToWidth,
  type ClickRegion,
  type HeaderPillSpec,
  type TuiTheme,
} from "../pi-grok-tui-kit/index.ts";
import { formatTimestamp, formatTokensM } from "../pi-grok-mentions/session-data.ts";
import {
  countTraceKinds,
  filterTraceEntries,
  formatTraceTime,
  type TraceEntry,
  type TraceFilter,
} from "./trace-data.ts";
import {
  renderPiCustomMessageElement,
  renderPiToolElement,
  type PiMessageRendererLike,
  type PiToolRenderersLike,
} from "./tool-render-tui.ts";

export interface TraceTuiOptions {
  entries: readonly TraceEntry[];
  sessionFile?: string;
  cwd?: string;
  initialFilter?: TraceFilter;
  initialQuery?: string;
  /** Optional `toolCallId` to pre-select when opened via Left-arrow on a collapsed tool trace. */
  initialToolCallId?: string;
  resolveToolRenderers?: (toolName: string) => PiToolRenderersLike | undefined;
  resolveMessageRenderer?: (customType: string) => PiMessageRendererLike | undefined;
  /** Optional clipboard copy callback (falls back to OSC 52 on stdout). */
  onCopy?: (text: string) => void;
  getRows?: () => number;
}

const FILTER_ORDER: readonly TraceFilter[] = [
  "all",
  "tool",
  "user",
  "assistant",
  "thinking",
  "meta",
];

const KIND_BADGE: Record<TraceEntry["kind"], { tag: string; color: string }> = {
  tool: { tag: "tool", color: "warning" },
  user: { tag: "user", color: "accent" },
  assistant: { tag: "agent", color: "success" },
  thinking: { tag: "think", color: "dim" },
  meta: { tag: "meta", color: "dim" },
};

const JSON_TOKEN_RE =
  /("(?:\\u[0-9a-fA-F]{4}|\\[^u]|[^\\"])*"(\s*:)?|\b(?:true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|[{}\[\],:])/g;

/** Lightweight theme-aware JSON line highlighter (preserves display width). */
export function highlightJsonLine(line: string, th: TuiTheme): string {
  return line.replace(JSON_TOKEN_RE, (match, _tok, isKey) => {
    if (isKey) {
      const colonIdx = match.lastIndexOf(":");
      const keyPart = match.slice(0, colonIdx);
      const tail = match.slice(colonIdx);
      return `${th.fg("accent", keyPart)}${th.fg("dim", tail)}`;
    }
    if (match.startsWith('"')) return th.fg("success", match);
    if (match === "true" || match === "false" || match === "null") return th.fg("warning", match);
    if (/^[{}\[\],:]$/.test(match)) return th.fg("dim", match);
    return th.fg("warning", match);
  });
}

/** Style wrapped preview lines for Pretty or Raw JSON mode. */
function styleTracePreviewLines(
  source: string,
  textW: number,
  mode: "pretty" | "raw",
  kind: TraceEntry["kind"],
  th: TuiTheme,
): string[] {
  const wrapped = wrapTextToWidth(source || "(empty)", textW);
  if (mode === "raw" || kind === "meta") {
    return wrapped.map((l) => ` ${highlightJsonLine(l, th)}`);
  }
  let inJson = false;
  let inCodeFence = false;
  return wrapped.map((l) => {
    const trimmed = l.trim();
    if (trimmed.startsWith("── ") && trimmed.endsWith(" ──")) {
      inJson = false;
      const color = trimmed.includes("(error)")
        ? "error"
        : trimmed.includes("Output")
          ? "success"
          : "accent";
      return ` ${styleBold(th, th.fg(color, l))}`;
    }
    if (trimmed.startsWith("```")) {
      inCodeFence = !inCodeFence;
      return ` ${th.fg("dim", l)}`;
    }
    if (!inCodeFence && (trimmed === "{" || trimmed === "[" || trimmed.startsWith('{"') || trimmed.startsWith('["'))) {
      inJson = true;
    }
    if (inJson) {
      const styled = ` ${highlightJsonLine(l, th)}`;
      if (trimmed === "}" || trimmed === "]" || trimmed === "}," || trimmed === "],") {
        inJson = false;
      }
      return styled;
    }
    if (l.startsWith("@@ ") && l.includes(" @@")) return ` ${th.fg("accent", l)}`;
    if (l.startsWith("+") && !l.startsWith("+++")) return ` ${th.fg("success", l)}`;
    if (l.startsWith("-") && !l.startsWith("---")) return ` ${th.fg("error", l)}`;
    if (trimmed.startsWith("#")) return ` ${styleBold(th, th.fg("accent", l))}`;
    if (inCodeFence) return ` ${th.fg("accent", l)}`;
    if (kind === "thinking") return ` ${th.fg("dim", l)}`;
    return ` ${l}`;
  });
}

export class TraceTui implements Component {
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly entries: readonly TraceEntry[];
  private readonly counts: Record<TraceFilter, number>;
  private readonly cwd: string;
  private readonly resolveToolRenderers?: (toolName: string) => PiToolRenderersLike | undefined;
  private readonly resolveMessageRenderer?: (customType: string) => PiMessageRendererLike | undefined;
  private readonly onCopy?: (text: string) => void;
  private readonly getRows: () => number;
  private readonly done: () => void;

  private filter: TraceFilter;
  private query: string;
  private viewMode: "pretty" | "pi" | "raw" = "pretty";
  private copyNotice = "";
  private filtered: TraceEntry[] = [];
  private readonly list = new ListController({ rowsPerItem: 1 });
  private previewScroll = 0;
  private lastPreviewTotalLines = 0;
  private previewCacheKey = "";
  private previewCacheLines: string[] = [];
  private readonly renderCoalescer: RenderCoalescer;
  private readonly dividerDrag = new DividerDragController(0.42);

  private lastTotalWidth = 0;
  private dividerX = 42;
  private bodyStartY = 5;
  private bodyHeight = 14;
  private pillRects = new Map<string, ClickRegion>();
  private closeRect: ClickRegion = { x1: -1, x2: -1, y: 1 };

  constructor(
    tui: { requestRender: () => void },
    theme: TuiTheme,
    options: TraceTuiOptions,
    done: () => void,
  ) {
    this.tui = tui;
    this.theme = theme;
    this.entries = options.entries;
    this.counts = countTraceKinds(options.entries);
    this.cwd = options.cwd ?? process.cwd();
    this.resolveToolRenderers = options.resolveToolRenderers;
    this.resolveMessageRenderer = options.resolveMessageRenderer;
    this.onCopy = options.onCopy;
    this.filter = options.initialFilter ?? "all";
    this.query = options.initialQuery ?? "";
    this.getRows = options.getRows ?? (() => 28);
    this.done = done;
    this.renderCoalescer = new RenderCoalescer(() => this.tui.requestRender());

    this.recompute(options.initialToolCallId);
  }

  getFilter(): TraceFilter {
    return this.filter;
  }

  getQuery(): string {
    return this.query;
  }

  getViewMode(): "pretty" | "pi" | "raw" {
    return this.viewMode;
  }

  getSelectedEntry(): TraceEntry | undefined {
    return this.filtered[this.list.selected];
  }

  invalidate(): void {
    // Re-rendered from live state.
  }

  private finish(): void {
    this.renderCoalescer.cancel();
    this.dividerDrag.cancel();
    this.done();
  }

  private recompute(focusCallIdOrEntryId?: string): void {
    const prevId = focusCallIdOrEntryId ?? this.filtered[this.list.selected]?.id;
    this.filtered = filterTraceEntries(this.entries, {
      filter: this.filter,
      query: this.query,
    });
    if (this.filtered.length === 0) {
      this.list.selected = 0;
      this.list.scroll = 0;
      this.previewScroll = 0;
      return;
    }
    if (prevId) {
      const idx = this.filtered.findIndex(
        (e) => e.toolCallId === prevId || e.id === prevId,
      );
      if (idx >= 0) {
        this.list.selected = idx;
      } else {
        this.list.selected = Math.min(this.list.selected, this.filtered.length - 1);
      }
    }
    this.list.clamp(this.filtered.length, this.bodyHeight);
    this.previewScroll = 0;
  }

  private setFilter(next: TraceFilter): void {
    if (this.filter === next) return;
    this.filter = next;
    this.recompute();
  }

  private cycleFilter(step = 1): void {
    const idx = FILTER_ORDER.indexOf(this.filter);
    const next = FILTER_ORDER[(idx + step + FILTER_ORDER.length) % FILTER_ORDER.length]!;
    this.setFilter(next);
  }

  private toggleViewMode(): void {
    this.viewMode =
      this.viewMode === "pretty" ? "pi" : this.viewMode === "pi" ? "raw" : "pretty";
    this.previewScroll = 0;
  }

  private copySelected(): void {
    const selected = this.filtered[this.list.selected];
    if (!selected) return;
    const payload = this.viewMode === "raw" ? selected.rawJson : selected.previewBody;
    if (!payload) return;
    if (this.onCopy) {
      this.onCopy(payload);
    } else {
      try {
        const b64 = Buffer.from(payload, "utf8").toString("base64");
        process.stdout.write(`\x1b]52;c;${b64}\x07`);
      } catch {
        // Ignore non-TTY stdout errors.
      }
    }
    this.copyNotice = `Copied #${selected.index}`;
    this.tui.requestRender();
  }

  private scrollPreview(delta: number): void {
    const maxScroll = Math.max(0, this.lastPreviewTotalLines - Math.max(1, this.bodyHeight - 5));
    this.previewScroll = Math.max(0, Math.min(maxScroll, this.previewScroll + delta));
  }

  handleInput(data: string): void {
    if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
      this.finish();
      return;
    }
    if (matchesKey(data, "tab")) {
      this.cycleFilter(1);
      return;
    }
    if (matchesKey(data, "shift+tab")) {
      this.cycleFilter(-1);
      return;
    }
    if (matchesKey(data, "ctrl+j")) {
      this.toggleViewMode();
      return;
    }
    if (matchesKey(data, "ctrl+x")) {
      this.copySelected();
      return;
    }
    if (matchesKey(data, "up")) {
      if (this.list.move(-1, this.filtered.length, this.bodyHeight)) {
        this.previewScroll = 0;
        this.copyNotice = "";
      }
      return;
    }
    if (matchesKey(data, "down")) {
      if (this.list.move(1, this.filtered.length, this.bodyHeight)) {
        this.previewScroll = 0;
        this.copyNotice = "";
      }
      return;
    }
    if (matchesKey(data, "home")) {
      if (this.list.selected !== 0) {
        this.list.selected = 0;
        this.list.clamp(this.filtered.length, this.bodyHeight);
        this.previewScroll = 0;
      }
      return;
    }
    if (matchesKey(data, "end")) {
      const last = Math.max(0, this.filtered.length - 1);
      if (this.list.selected !== last) {
        this.list.selected = last;
        this.list.clamp(this.filtered.length, this.bodyHeight);
        this.previewScroll = 0;
      }
      return;
    }
    if (matchesKey(data, "pageUp") || matchesKey(data, "ctrl+u")) {
      this.scrollPreview(-Math.max(4, Math.floor(this.bodyHeight / 2)));
      return;
    }
    if (matchesKey(data, "pageDown") || matchesKey(data, "ctrl+d")) {
      this.scrollPreview(Math.max(4, Math.floor(this.bodyHeight / 2)));
      return;
    }
    if (matchesKey(data, "ctrl+w") || matchesKey(data, "ctrl+l")) {
      if (this.query.length > 0) {
        this.query = "";
        this.recompute();
      }
      return;
    }
    if (matchesKey(data, "backspace")) {
      if (this.query.length > 0) {
        this.query = Array.from(this.query).slice(0, -1).join("");
        this.recompute();
      }
      return;
    }

    let printable = data;
    if (printable.startsWith("\x1b[200~") && printable.endsWith("\x1b[201~")) {
      printable = printable.slice(6, -6).replace(/[\r\n]+/g, " ");
    }
    if (!printable.startsWith("\x1b") && !/[\x00-\x1f\x7f]/.test(printable)) {
      this.query += printable;
      this.recompute();
    }
  }

  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    const { x, y, type } = event;
    const dragResult = this.dividerDrag.handleMouse(event, {
      totalWidth: this.lastTotalWidth,
      dividerX: this.dividerX,
      bodyStartY: this.bodyStartY,
      bodyHeight: this.bodyHeight,
      minLeft: 26,
      minRight: 28,
    });
    if (dragResult) return dragResult;

    if (type === "wheel") {
      const delta = (event.wheelDelta ?? 0) > 0 ? 1 : -1;
      if (y >= this.bodyStartY && y < this.bodyStartY + this.bodyHeight) {
        if (x < this.dividerX) {
          if (this.list.move(delta, this.filtered.length, this.bodyHeight)) {
            this.previewScroll = 0;
          }
          return { handled: true, render: true };
        }
        this.scrollPreview(delta * 3);
        this.renderCoalescer.schedule();
        return { handled: true, render: false };
      }
      return { handled: true, render: false };
    }

    if (type === "press" && event.button === "left") {
      if (hitRegion(this.closeRect, x, y)) {
        this.finish();
        return { handled: true, render: false };
      }
      for (const [id, rect] of this.pillRects) {
        if (!hitRegion(rect, x, y)) continue;
        if (id === "view:toggle") {
          this.toggleViewMode();
          return { handled: true, render: true };
        }
        if (FILTER_ORDER.includes(id as TraceFilter)) {
          this.setFilter(id as TraceFilter);
          return { handled: true, render: true };
        }
      }
      if (
        y >= this.bodyStartY &&
        y < this.bodyStartY + this.bodyHeight &&
        x >= 1 &&
        x < this.dividerX
      ) {
        const idx = this.list.indexAtBodyRow(y - this.bodyStartY, this.filtered.length);
        if (idx >= 0 && idx < this.filtered.length && idx !== this.list.selected) {
          this.list.selected = idx;
          this.list.clamp(this.filtered.length, this.bodyHeight);
          this.previewScroll = 0;
          return { handled: true, render: true };
        }
      }
      return { handled: true, render: false };
    }

    return { handled: true, render: false };
  }

  private renderLeftPane(leftWidth: number, height: number): string[] {
    const th = this.theme;
    this.list.clamp(this.filtered.length, height);
    if (this.filtered.length === 0) {
      const empty = [
        "",
        th.fg("dim", "  No matching trace entries"),
        th.fg("dim", "  Press Tab to switch filter or Backspace to clear search"),
      ];
      while (empty.length < height) empty.push("");
      return empty.slice(0, height).map((line) => fitWidth(line, leftWidth));
    }

    const lines: string[] = [];
    const perPage = this.list.itemsPerPage(height);
    const hl = (m: string) => `\x1b[7m${m}\x1b[27m`;
    for (let slot = 0; slot < perPage; slot++) {
      const idx = this.list.scroll + slot;
      if (idx >= this.filtered.length) break;
      const entry = this.filtered[idx]!;
      const selected = idx === this.list.selected;
      const badge = KIND_BADGE[entry.kind];
      const marker = selected ? th.fg("accent", "▸ ") : "  ";
      const time = formatTraceTime(entry.timestamp);
      const rightTime = time ? ` ${time}` : "";
      const numPrefix = `#${entry.index} `;
      const tagRaw = `[${badge.tag}] `;
      const errRaw = entry.isError ? "✗ " : "";
      const labelBudget = Math.max(
        6,
        leftWidth -
          2 -
          visibleWidth(numPrefix) -
          visibleWidth(tagRaw) -
          visibleWidth(errRaw) -
          visibleWidth(rightTime) -
          1,
      );
      const labelTrunc = truncateToWidth(entry.label, labelBudget, "…");
      const labelStyled = selected
        ? styleBold(th, th.fg("accent", labelTrunc))
        : this.query.trim()
          ? highlightTerms(labelTrunc, this.query, hl)
          : labelTrunc;
      const tagStyled = styleBold(th, th.fg(badge.color, tagRaw));
      const errStyled = errRaw ? styleBold(th, th.fg("error", errRaw)) : "";
      const leftRow1 = `${marker}${th.fg("dim", numPrefix)}${tagStyled}${errStyled}${labelStyled}`;
      const gap = Math.max(0, leftWidth - visibleWidth(leftRow1) - visibleWidth(rightTime));
      const row1 = `${leftRow1}${" ".repeat(gap)}${th.fg("dim", rightTime)}`;

      lines.push(fitWidth(row1, leftWidth));
    }

    while (lines.length < height) lines.push(" ".repeat(leftWidth));
    return lines.slice(0, height);
  }

  private renderRightPane(rightWidth: number, height: number): string[] {
    const th = this.theme;
    const selected = this.filtered[this.list.selected];
    if (!selected) {
      const empty = [
        "",
        th.fg("dim", "  No trace entry selected."),
      ];
      while (empty.length < height) empty.push("");
      return empty.slice(0, height).map((line) => fitWidth(line, rightWidth));
    }

    const pad = (s: string) => ` ${s}`;
    const badge = KIND_BADGE[selected.kind];
    const headerLines: string[] = [
      pad(
        `${styleBold(th, th.fg("accent", selected.label))}${this.copyNotice ? `  ${th.fg("success", `✓ ${this.copyNotice}`)}` : ""}`,
      ),
      pad(
        `${th.fg("dim", "Entry:")} #${selected.index}  ${th.fg("dim", "Line:")} ${selected.lineNumber}  ${th.fg("dim", "Type:")} ${styleBold(th, th.fg(badge.color, selected.rawType))}  ${th.fg("dim", "View:")} ${this.viewMode === "raw" ? th.fg("warning", "Raw JSON") : this.viewMode === "pi" ? th.fg("accent", "Pi Element") : th.fg("success", "Pretty")}`,
      ),
    ];
    const metaParts: string[] = [];
    if (selected.toolCallId) metaParts.push(`${th.fg("dim", "Call:")} ${selected.toolCallId}`);
    if (selected.timestamp) metaParts.push(`${th.fg("dim", "Time:")} ${formatTimestamp(selected.timestamp)}`);
    if (selected.model) metaParts.push(`${th.fg("dim", "Model:")} ${th.fg("success", selected.model.split("/").pop()!)}`);
    if (selected.usage) {
      const total =
        selected.usage.input +
        selected.usage.output +
        selected.usage.cacheRead +
        selected.usage.cacheWrite;
      metaParts.push(`${th.fg("dim", "Tokens:")} ${th.fg("warning", formatTokensM(total))}`);
    }
    if (metaParts.length > 0) {
      headerLines.push(pad(metaParts.join("  ")));
    }
    headerLines.push(th.fg("border", "─".repeat(rightWidth)));

    const bodyHeight = Math.max(1, height - headerLines.length);
    const textW = Math.max(10, rightWidth - 2);
    const cacheKey = `${selected.index}:${selected.id}:${textW}:${this.viewMode}`;
    if (cacheKey !== this.previewCacheKey) {
      this.previewCacheKey = cacheKey;
      let renderedPi: string[] | null = null;
      if (this.viewMode === "pi") {
        if (selected.kind === "tool" && selected.toolName) {
          renderedPi = renderPiToolElement({
            invocation: {
              toolCallId: selected.toolCallId ?? selected.id,
              toolName: selected.toolName,
              args: selected.toolArgs ?? selected.toolInput ?? {},
              result: selected.toolResultPayload,
              isError: selected.isError === true,
              timestamp: selected.timestamp,
            },
            width: rightWidth,
            theme: this.theme,
            cwd: this.cwd,
            resolveRenderers: this.resolveToolRenderers,
            onInvalidate: () => {
              this.previewCacheKey = "";
              this.tui.requestRender();
            },
          });
        } else if (selected.customType && selected.customMessage) {
          renderedPi = renderPiCustomMessageElement({
            customType: selected.customType,
            message: selected.customMessage,
            width: rightWidth,
            theme: this.theme,
            resolveMessageRenderer: this.resolveMessageRenderer,
          });
        }
      }
      if (renderedPi) {
        this.previewCacheLines = renderedPi;
      } else {
        const source = this.viewMode === "raw" ? selected.rawJson : selected.previewBody;
        this.previewCacheLines = styleTracePreviewLines(
          source,
          textW,
          this.viewMode === "raw" ? "raw" : "pretty",
          selected.kind,
          th,
        );
      }
    }

    this.lastPreviewTotalLines = this.previewCacheLines.length;
    const maxScroll = Math.max(0, this.previewCacheLines.length - bodyHeight);
    if (this.previewScroll > maxScroll) this.previewScroll = maxScroll;

    const hl = (m: string) => `\x1b[7m${m}\x1b[27m`;
    const visibleBody = this.previewCacheLines
      .slice(this.previewScroll, this.previewScroll + bodyHeight)
      .map((line) => (this.query.trim() ? highlightTerms(line, this.query, hl) : line));
    while (visibleBody.length < bodyHeight) visibleBody.push("");

    return [...headerLines, ...visibleBody]
      .slice(0, height)
      .map((line) => fitWidth(line, rightWidth));
  }

  render(width: number): string[] {
    const termRows = this.getRows();
    const targetRows = Math.max(16, Math.floor(termRows * 0.9));
    const geometry = computeSplitGeometry({
      width,
      rows: termRows,
      leftRatio: this.dividerDrag.leftRatio,
      hasSearchBar: true,
      minWidth: 64,
      minLeftWidth: 26,
      minRightWidth: 28,
      heightRatio: 0.9,
      maxRows: targetRows,
    });
    this.lastTotalWidth = geometry.totalWidth;
    this.dividerX = geometry.dividerX;
    this.bodyStartY = geometry.bodyStartY;
    this.bodyHeight = geometry.bodyHeight;

    const pills: HeaderPillSpec[] = [
      { id: "all", label: `[All ${this.counts.all}]`, active: this.filter === "all" },
      { id: "tool", label: `[Tools ${this.counts.tool}]`, active: this.filter === "tool" },
      { id: "user", label: `[User ${this.counts.user}]`, active: this.filter === "user" },
      { id: "assistant", label: `[Agent ${this.counts.assistant}]`, active: this.filter === "assistant" },
      { id: "thinking", label: `[Think ${this.counts.thinking}]`, active: this.filter === "thinking" },
      { id: "meta", label: `[Meta ${this.counts.meta}]`, active: this.filter === "meta" },
      {
        id: "view:toggle",
        label:
          this.viewMode === "raw"
            ? "[Raw JSON]"
            : this.viewMode === "pi"
              ? "[Pi Element]"
              : "[Pretty]",
        active: this.viewMode !== "pretty",
        color: this.viewMode === "raw" ? "warning" : this.viewMode === "pi" ? "accent" : "dim",
      },
    ];

    const frame = renderSplitOverlayFrame({
      geometry,
      titlePrefix: ` ${styleBold(this.theme, this.theme.fg("accent", "/trace"))} `,
      pills,
      search: {
        promptPrefix: ` ${this.theme.fg("accent", "Search:")} `,
        query: this.query,
        placeholder: "filter tool name, call id, args, output, jsonl...",
        rightLabel: `${this.filtered.length}/${this.entries.length} entries `,
      },
      leftLines: this.renderLeftPane(geometry.leftWidth, geometry.bodyHeight),
      rightLines: this.renderRightPane(geometry.rightWidth, geometry.bodyHeight),
      footerHints: [
        { key: "↑↓/Wheel", label: "select" },
        { key: "Tab/Click", label: "filter" },
        { key: "Ctrl+J", label: "Pretty/Pi/Raw" },
        { key: "Ctrl+X", label: "copy" },
        { key: "PgUp/PgDn", label: "scroll" },
        { key: "Drag │", label: "resize" },
        { key: "Esc", label: "close" },
      ],
      theme: this.theme,
    });
    this.pillRects = frame.pillRects;
    this.closeRect = frame.closeRect;
    return frame.lines;
  }
}

export function createTraceTui(
  tui: { requestRender: () => void },
  theme: TuiTheme,
  options: TraceTuiOptions,
  done: () => void,
): TraceTui {
  return new TraceTui(tui, theme, options, done);
}
