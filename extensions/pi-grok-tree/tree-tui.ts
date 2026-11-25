/**
 * Bordered two-pane TS TUI overlay for `/tree` session branch navigation.
 *
 * Built on `../pi-grok-tui-kit/` (`renderSplitOverlayFrame`, `ListController`,
 * `hitRegion`, `fitWidth`, `wrapTextToWidth`, `highlightTerms`).
 */

import {
  matchesKey,
  parseKey,
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
  renderSplitOverlayFrame,
  RenderCoalescer,
  styleBold,
  wrapTextToWidth,
  type ClickRegion,
  type HeaderPillSpec,
  type TuiTheme,
} from "../pi-grok-tui-kit/index.ts";
import {
  filterTreeNodes,
  type FlattenedTreeNode,
  type TreeFilterMode,
} from "./tree-data.ts";

export type SessionTreeActionResult =
  | {
      action: "navigate";
      entryId: string;
      summarize: boolean;
    }
  | {
      action: "label";
      entryId: string;
      label: string;
    };

export interface SessionTreeTuiOptions {
  nodes: readonly FlattenedTreeNode[];
  initialFilter?: TreeFilterMode;
  initialQuery?: string;
  /** Optional clipboard copy callback (falls back to OSC 52 on stdout). */
  onCopy?: (text: string) => void;
  getRows?: () => number;
}

const FILTER_CYCLE: readonly TreeFilterMode[] = [
  "default",
  "no-tools",
  "user-only",
  "labeled",
  "all",
];

const SGR_MOUSE_RE = /^\x1b\[<(\d+);(\d+);(\d+)([mM])$/;

class SessionTreeComponent implements Component {
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly allNodes: readonly FlattenedTreeNode[];
  private readonly onCopy?: (text: string) => void;
  private readonly getRows: () => number;
  private readonly done: (result: SessionTreeActionResult | undefined) => void;

  private filter: TreeFilterMode;
  private query: string;
  private showTimestamps = false;
  private labelEditing = false;
  private labelDraft = "";
  private statusNotice = "";
  private filtered: FlattenedTreeNode[] = [];
  private readonly list = new ListController({ rowsPerItem: 1 });
  private previewScroll = 0;
  private readonly previewLinesCache = new Map<string, string[]>();
  private previewTargetId: string | undefined;
  private readonly renderCoalescer: RenderCoalescer;
  private readonly dividerDrag = new DividerDragController(0.45);

  private lastTotalWidth = 100;
  private lastBodyHeight = 14;
  private lastBodyStartY = 5;
  private lastDividerX = 44;
  private lastPillRects = new Map<string, ClickRegion>();
  private lastCloseRect: ClickRegion | undefined;

  constructor(
    tui: { requestRender: () => void },
    theme: TuiTheme,
    options: SessionTreeTuiOptions,
    done: (result: SessionTreeActionResult | undefined) => void,
  ) {
    this.tui = tui;
    this.renderCoalescer = new RenderCoalescer(() => this.tui.requestRender());
    this.theme = theme;
    this.allNodes = options.nodes;
    this.filter = options.initialFilter ?? "default";
    this.query = options.initialQuery ?? "";
    this.onCopy = options.onCopy;
    this.getRows = options.getRows ?? (() => 28);
    this.done = done;

    this.recomputeFilter(false);
    // Focus the active leaf entry on initial open when visible.
    const leafIdx = this.filtered.findIndex((n) => n.isLeaf);
    if (leafIdx >= 0) {
      this.list.select(leafIdx, this.filtered.length, this.lastBodyHeight);
    }
  }

  invalidate(): void {
    // Recomputed on render().
  }

  private recomputeFilter(resetPreview = true): void {
    const prevId = this.filtered[this.list.selected]?.entry.id;
    this.filtered = filterTreeNodes(this.allNodes, this.filter, this.query);
    if (this.filtered.length === 0) {
      this.list.reset();
      this.previewScroll = 0;
      return;
    }
    const kept = prevId ? this.filtered.findIndex((n) => n.entry.id === prevId) : -1;
    if (kept >= 0) {
      this.list.select(kept, this.filtered.length, this.lastBodyHeight);
    } else {
      this.list.clamp(this.filtered.length, this.lastBodyHeight);
      if (resetPreview) this.previewScroll = 0;
    }
  }

  private finish(result: SessionTreeActionResult | undefined): void {
    this.renderCoalescer.cancel();
    this.done(result);
  }

  private confirmSelected(summarize: boolean): void {
    const node = this.filtered[this.list.selected];
    if (!node) return;
    this.finish({ action: "navigate", entryId: node.entry.id, summarize });
  }

  private jumpToBranch(direction: -1 | 1): void {
    const len = this.filtered.length;
    if (len === 0) return;
    let idx = this.list.selected + direction;
    while (idx >= 0 && idx < len) {
      if (this.filtered[idx]!.isBranchPoint) {
        this.list.select(idx, len, this.lastBodyHeight);
        this.previewScroll = 0;
        return;
      }
      idx += direction;
    }
  }

  private copySelected(): void {
    const node = this.filtered[this.list.selected];
    if (!node) return;
    const text = node.entry.fullText || node.entry.summary || node.entry.id;
    if (this.onCopy) {
      this.onCopy(text);
    } else if (process.stdout?.isTTY) {
      const b64 = Buffer.from(text, "utf8").toString("base64");
      process.stdout.write(`\x1b]52;c;${b64}\x07`);
    }
    this.statusNotice = `Copied ${node.entry.id.slice(0, 8)}`;
  }

  private buildLeftLines(leftWidth: number, bodyHeight: number): string[] {
    const th = this.theme;
    const pad = (s: string) => fitWidth(s, leftWidth);
    if (this.filtered.length === 0) {
      return [pad(""), pad(th.fg("muted", "  No matching tree entries"))];
    }

    const highlightMatch = (m: string) => styleBold(th, th.fg("warning", m));
    const end = Math.min(this.filtered.length, this.list.scroll + bodyHeight);
    const lines: string[] = [];

    for (let i = this.list.scroll; i < end; i++) {
      const node = this.filtered[i]!;
      const selected = i === this.list.selected;
      const pointer = selected ? th.fg("accent", "▸") : " ";
      const dot = node.isLeaf
        ? th.fg("warning", "★")
        : node.isOnActivePath
          ? th.fg("success", "●")
          : th.fg("dim", "○");
      const roleColor =
        node.entry.kind === "user"
          ? "accent"
          : node.entry.kind === "assistant"
            ? "success"
            : node.entry.kind === "branch"
              ? "warning"
              : "muted";
      const labelTag = node.entry.label ? `${th.fg("warning", `[${node.entry.label}]`)} ` : "";
      const timeTag =
        this.showTimestamps && node.entry.timestamp
          ? `${th.fg("dim", node.entry.timestamp)} `
          : "";
      const text = `${node.entry.roleLabel}: ${node.entry.summary}`;
      const highlighted = selected
        ? styleBold(th, th.fg(roleColor, text))
        : th.fg(roleColor, highlightTerms(text, this.query, highlightMatch));
      const row = `${pointer}${dot} ${th.fg("dim", node.prefix)}${timeTag}${labelTag}${highlighted}`;
      lines.push(pad(row));
    }
    return lines;
  }

  private buildPreviewParts(
    node: FlattenedTreeNode | undefined,
    rightWidth: number,
  ): { header: string[]; body: string[] } {
    const th = this.theme;
    const pad = (s: string) => fitWidth(s, rightWidth);
    if (!node) {
      return {
        header: [pad(""), pad(th.fg("muted", "  Select a node to preview its content."))],
        body: [],
      };
    }

    const status = node.isLeaf
      ? th.fg("warning", "★ Current Leaf")
      : node.isOnActivePath
        ? th.fg("success", "● Active Branch")
        : th.fg("dim", "○ Alternate Branch");
    const header = [
      pad(`  ${styleBold(th, th.fg("accent", node.entry.roleLabel))}  ${status}  ${th.fg("dim", node.entry.timestamp)}`),
      pad(`  ${th.fg("muted", "ID:")} ${th.fg("text", node.entry.id)}${node.entry.label ? `   ${th.fg("muted", "Label:")} ${th.fg("warning", node.entry.label)}` : ""}`),
      pad(th.fg("borderMuted", `  ${"─".repeat(Math.max(8, rightWidth - 4))}`)),
    ];

    const cacheKey = `${node.entry.id}:${rightWidth}`;
    let body = this.previewLinesCache.get(cacheKey);
    if (!body) {
      const bodyW = Math.max(12, rightWidth - 4);
      body = [];
      for (const rawLine of node.entry.fullText.split(/\r?\n/)) {
        for (const seg of wrapTextToWidth(rawLine, bodyW)) {
          body.push(pad(`  ${seg}`));
        }
      }
      this.previewLinesCache.set(cacheKey, body);
    }
    return { header, body };
  }

  render(width: number): string[] {
    const termRows = this.getRows();
    const targetRows = Math.max(16, Math.floor(termRows * 0.9));
    const geom = computeSplitGeometry({
      width,
      rows: termRows,
      leftRatio: this.dividerDrag.leftRatio,
      hasSearchBar: true,
      minLeftWidth: 28,
      heightRatio: 0.9,
      maxRows: targetRows,
    });
    this.lastTotalWidth = geom.totalWidth;
    this.lastBodyHeight = geom.bodyHeight;
    this.lastBodyStartY = geom.bodyStartY;
    this.lastDividerX = geom.dividerX;
    this.list.clamp(this.filtered.length, geom.bodyHeight);

    const pills: HeaderPillSpec[] = [
      { id: "default", label: "[Default]", active: this.filter === "default" },
      { id: "no-tools", label: "[NoTools]", active: this.filter === "no-tools" },
      { id: "user-only", label: "[User]", active: this.filter === "user-only" },
      { id: "labeled", label: "[Labeled]", active: this.filter === "labeled" },
      { id: "all", label: "[All]", active: this.filter === "all" },
    ];

    const leftLines = this.buildLeftLines(geom.leftWidth, geom.bodyHeight);
    const active = this.filtered[this.list.selected];
    const targetId = active?.entry.id;
    if (targetId !== this.previewTargetId) {
      this.previewLinesCache.clear();
      this.previewTargetId = targetId;
    }
    const preview = this.buildPreviewParts(active, geom.rightWidth);
    const maxPreviewScroll = Math.max(
      0,
      preview.header.length + preview.body.length - geom.bodyHeight,
    );
    this.previewScroll = Math.max(0, Math.min(this.previewScroll, maxPreviewScroll));
    const highlightMatch = (m: string) => styleBold(this.theme, this.theme.fg("warning", m));
    const rightLines = [
      ...preview.header.slice(this.previewScroll, this.previewScroll + geom.bodyHeight),
      ...preview.body
        .slice(
          Math.max(0, this.previewScroll - preview.header.length),
          this.previewScroll + geom.bodyHeight - preview.header.length,
        )
        .map((line) => highlightTerms(line, this.query, highlightMatch)),
    ];

    const searchBar = this.labelEditing
      ? {
          promptPrefix: ` ${this.theme.fg("warning", "Label:")} `,
          query: this.labelDraft,
          placeholder: "enter label text (Enter save, Esc cancel)...",
          rightLabel: "editing label",
        }
      : {
          promptPrefix: ` ${this.theme.fg("accent", "/")} `,
          query: this.query,
          placeholder: "search tree nodes, messages, or labels...",
          rightLabel: this.statusNotice
            ? `${this.statusNotice} · (${this.filtered.length}/${this.allNodes.length})`
            : `(${this.filtered.length}/${this.allNodes.length})`,
        };

    const frame = renderSplitOverlayFrame({
      geometry: geom,
      titlePrefix: ` ${styleBold(this.theme, this.theme.fg("accent", "/tree"))} `,
      pills,
      search: searchBar,
      leftLines,
      rightLines,
      footerHints: [
        { key: "↑/↓", label: "move" },
        { key: "←/→", label: "page" },
        { key: "Alt+←/→", label: "branch" },
        { key: "Ctrl+X", label: "copy" },
        { key: "Shift+L", label: "label" },
        { key: "Shift+T", label: "time" },
        { key: "Ctrl+O", label: "filter" },
        { key: "Enter", label: "jump" },
        { key: "Ctrl+S", label: "summary" },
        { key: "Esc", label: "close" },
      ],
      theme: this.theme,
    });
    this.lastPillRects = frame.pillRects;
    this.lastCloseRect = frame.closeRect;
    return frame.lines;
  }

  /** Native Pi TUI mouse handler (dispatched by `pi-grok-remote-tui`). */
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    const { x, y, type } = event;
    const dragRes = this.dividerDrag.handleMouse(event, {
      totalWidth: this.lastTotalWidth,
      dividerX: this.lastDividerX,
      bodyStartY: this.lastBodyStartY,
      bodyHeight: this.lastBodyHeight,
      minLeft: 28,
    });
    if (dragRes) return dragRes;
    if (type === "wheel") {
      const delta = (event.wheelDelta ?? 0) > 0 ? 1 : -1;
      if (y >= this.lastBodyStartY && y < this.lastBodyStartY + this.lastBodyHeight) {
        if (x > this.lastDividerX) {
          this.previewScroll = Math.max(0, this.previewScroll + delta * 3);
          this.renderCoalescer.schedule();
          return { handled: true, render: false };
        }
        if (this.list.move(delta, this.filtered.length, this.lastBodyHeight)) {
          this.previewScroll = 0;
        }
        return { handled: true, render: true };
      }
      return { handled: true, render: false };
    }

    if (type === "press" && event.button === "left") {
      if (this.lastCloseRect && hitRegion(this.lastCloseRect, x, y)) {
        this.finish(undefined);
        return { handled: true, render: false };
      }
      for (const [id, rect] of this.lastPillRects) {
        if (hitRegion(rect, x, y)) {
          const next = id as TreeFilterMode;
          if (next !== this.filter) {
            this.filter = next;
            this.recomputeFilter(true);
          }
          return { handled: true, render: true };
        }
      }
      if (
        y >= this.lastBodyStartY &&
        y < this.lastBodyStartY + this.lastBodyHeight &&
        x >= 1 &&
        x < this.lastDividerX
      ) {
        const idx = this.list.indexAtBodyRow(y - this.lastBodyStartY, this.filtered.length);
        if (idx >= 0 && idx < this.filtered.length) {
          this.list.select(idx, this.filtered.length, this.lastBodyHeight);
          this.previewScroll = 0;
          return { handled: true, render: true };
        }
      }
      return { handled: true, render: false };
    }

    if (type === "click" && event.button === "left" && (event.clickCount ?? 1) >= 2) {
      if (
        y >= this.lastBodyStartY &&
        y < this.lastBodyStartY + this.lastBodyHeight &&
        x >= 1 &&
        x < this.lastDividerX
      ) {
        const idx = this.list.indexAtBodyRow(y - this.lastBodyStartY, this.filtered.length);
        if (idx >= 0 && idx < this.filtered.length) {
          this.list.select(idx, this.filtered.length, this.lastBodyHeight);
          this.confirmSelected(false);
          return { handled: true, render: false };
        }
      }
    }

    return { handled: true, render: false };
  }

  private handleSgrMouse(data: string): boolean {
    const m = SGR_MOUSE_RE.exec(data);
    if (!m) return false;
    const btn = Number(m[1]);
    const col = Number(m[2]) - 1;
    const row = Number(m[3]) - 1;
    const isPress = m[4] === "M";

    if (btn === 64 || btn === 65) {
      const delta = btn === 64 ? -1 : 1;
      if (col > this.lastDividerX) {
        this.previewScroll = Math.max(0, this.previewScroll + delta * 3);
        this.renderCoalescer.schedule();
      } else {
        if (this.list.move(delta, this.filtered.length, this.lastBodyHeight)) {
          this.previewScroll = 0;
        }
        this.tui.requestRender();
      }
      return true;
    }

    if (!isPress || (btn & 3) !== 0) return true;

    if (this.lastCloseRect && hitRegion(this.lastCloseRect, col, row)) {
      this.finish(undefined);
      return true;
    }

    for (const [id, rect] of this.lastPillRects) {
      if (hitRegion(rect, col, row)) {
        const next = id as TreeFilterMode;
        if (next !== this.filter) {
          this.filter = next;
          this.recomputeFilter(true);
          this.tui.requestRender();
        }
        return true;
      }
    }

    if (
      row >= this.lastBodyStartY &&
      row < this.lastBodyStartY + this.lastBodyHeight &&
      col >= 1 &&
      col < this.lastDividerX
    ) {
      const idx = this.list.indexAtBodyRow(row - this.lastBodyStartY, this.filtered.length);
      const outcome = this.list.handleClick(idx, this.filtered.length, this.lastBodyHeight);
      if (outcome === "confirm") {
        this.confirmSelected(false);
        return true;
      }
      if (outcome === "select") {
        this.previewScroll = 0;
        this.tui.requestRender();
      }
      return true;
    }

    return true;
  }

  handleInput(data: string): void {
    if (this.handleSgrMouse(data)) return;
    this.statusNotice = "";

    if (matchesKey(data, "ctrl+c")) {
      this.finish(undefined);
      return;
    }
    if (this.labelEditing) {
      if (matchesKey(data, "escape")) {
        this.labelEditing = false;
        this.labelDraft = "";
        this.tui.requestRender();
        return;
      }
      if (matchesKey(data, "enter") || matchesKey(data, "return")) {
        const node = this.filtered[this.list.selected];
        const label = this.labelDraft.trim();
        this.labelEditing = false;
        this.labelDraft = "";
        if (node) {
          this.finish({ action: "label", entryId: node.entry.id, label });
        }
        return;
      }
      if (matchesKey(data, "backspace")) {
        this.labelDraft = this.labelDraft.slice(0, -1);
        this.tui.requestRender();
        return;
      }
      if (data.length > 0 && !data.startsWith("\x1b")) {
        const printable = [...data].filter((ch) => ch >= " " && ch !== "\x7f").join("");
        if (printable) {
          this.labelDraft += printable;
          this.tui.requestRender();
        }
      }
      return;
    }
    if (matchesKey(data, "escape")) {
      if (this.query.length > 0) {
        this.query = "";
        this.recomputeFilter(true);
        this.tui.requestRender();
        return;
      }
      this.finish(undefined);
      return;
    }
    if (matchesKey(data, "enter") || matchesKey(data, "return")) {
      this.confirmSelected(false);
      return;
    }
    if (matchesKey(data, "ctrl+s")) {
      this.confirmSelected(true);
      return;
    }
    if (matchesKey(data, "ctrl+x")) {
      this.copySelected();
      this.tui.requestRender();
      return;
    }
    // Upstream pi-main Session Tree shortcuts: Shift+L (label) & Shift+T (timestamps)
    // when search query is empty so normal uppercase typing in search still works.
    if (this.query.length === 0 && (data === "L" || matchesKey(data, "alt+l"))) {
      const node = this.filtered[this.list.selected];
      if (node) {
        this.labelEditing = true;
        this.labelDraft = node.entry.label ?? "";
        this.tui.requestRender();
      }
      return;
    }
    if (this.query.length === 0 && (data === "T" || matchesKey(data, "alt+t"))) {
      this.showTimestamps = !this.showTimestamps;
      this.tui.requestRender();
      return;
    }
    // Option+←/→ (Alt+Left/Right): jump between branch points.
    if (matchesKey(data, "alt+left")) {
      this.jumpToBranch(-1);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "alt+right")) {
      this.jumpToBranch(1);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "home")) {
      if (this.filtered.length > 0) {
        this.list.select(0, this.filtered.length, this.lastBodyHeight);
        this.previewScroll = 0;
        this.tui.requestRender();
      }
      return;
    }
    if (matchesKey(data, "end")) {
      if (this.filtered.length > 0) {
        this.list.select(this.filtered.length - 1, this.filtered.length, this.lastBodyHeight);
        this.previewScroll = 0;
        this.tui.requestRender();
      }
      return;
    }
    // Direct filter chords matching Rust SessionTree (Ctrl+D/T/U/L/A & Ctrl+O cycle).
    if (matchesKey(data, "ctrl+shift+o") || matchesKey(data, "shift+tab")) {
      const idx = (FILTER_CYCLE.indexOf(this.filter) - 1 + FILTER_CYCLE.length) % FILTER_CYCLE.length;
      this.filter = FILTER_CYCLE[idx]!;
      this.recomputeFilter(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "ctrl+o") || matchesKey(data, "tab")) {
      const idx = (FILTER_CYCLE.indexOf(this.filter) + 1) % FILTER_CYCLE.length;
      this.filter = FILTER_CYCLE[idx]!;
      this.recomputeFilter(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "ctrl+d")) {
      this.filter = "default";
      this.recomputeFilter(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "ctrl+t")) {
      this.filter = "no-tools";
      this.recomputeFilter(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "ctrl+u")) {
      this.filter = "user-only";
      this.recomputeFilter(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "ctrl+l")) {
      this.filter = "labeled";
      this.recomputeFilter(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "ctrl+a")) {
      this.filter = "all";
      this.recomputeFilter(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "up") || matchesKey(data, "ctrl+p")) {
      if (this.list.move(-1, this.filtered.length, this.lastBodyHeight)) this.previewScroll = 0;
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "down") || matchesKey(data, "ctrl+n")) {
      if (this.list.move(1, this.filtered.length, this.lastBodyHeight)) this.previewScroll = 0;
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "pageUp") || matchesKey(data, "left")) {
      if (this.list.page(-1, this.filtered.length, this.lastBodyHeight)) this.previewScroll = 0;
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "pageDown") || matchesKey(data, "right")) {
      if (this.list.page(1, this.filtered.length, this.lastBodyHeight)) this.previewScroll = 0;
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "backspace")) {
      if (this.query.length > 0) {
        this.query = this.query.slice(0, -1);
        this.recomputeFilter(true);
        this.tui.requestRender();
      }
      return;
    }
    if (matchesKey(data, "ctrl+w")) {
      this.query = "";
      this.recomputeFilter(true);
      this.tui.requestRender();
      return;
    }

    const parsed = parseKey(data);
    if (!parsed && data.length > 0 && !data.startsWith("\x1b")) {
      const printable = [...data].filter((ch) => ch >= " " && ch !== "\x7f").join("");
      if (printable) {
        this.query += printable;
        this.recomputeFilter(true);
        this.tui.requestRender();
      }
    }
  }
}

export function createSessionTreeTui(
  tui: { requestRender: () => void },
  theme: TuiTheme,
  options: SessionTreeTuiOptions,
  done: (result: SessionTreeActionResult | undefined) => void,
): Component {
  return new SessionTreeComponent(tui, theme, options, done);
}
