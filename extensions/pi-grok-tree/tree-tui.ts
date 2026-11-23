/**
 * Bordered two-pane TS TUI overlay for `/tree` session branch navigation.
 *
 * Built on `../pi-grok-tui-kit/` (`renderSplitOverlayFrame`, `ListController`,
 * `hitRegion`, `fitWidth`, `wrapTextToWidth`, `highlightTerms`).
 */

import { matchesKey, parseKey, type Component } from "@earendil-works/pi-tui";
import {
  computeSplitGeometry,
  fitWidth,
  highlightTerms,
  hitRegion,
  ListController,
  renderSplitOverlayFrame,
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

export interface SessionTreeActionResult {
  action: "navigate";
  entryId: string;
  summarize: boolean;
}

export interface SessionTreeTuiOptions {
  nodes: readonly FlattenedTreeNode[];
  initialFilter?: TreeFilterMode;
  initialQuery?: string;
  getRows?: () => number;
}

const SGR_MOUSE_RE = /^\x1b\[<(\d+);(\d+);(\d+)([mM])$/;

class SessionTreeComponent implements Component {
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly allNodes: readonly FlattenedTreeNode[];
  private readonly getRows: () => number;
  private readonly done: (result: SessionTreeActionResult | undefined) => void;

  private filter: TreeFilterMode;
  private query: string;
  private filtered: FlattenedTreeNode[] = [];
  private readonly list = new ListController({ rowsPerItem: 1 });
  private previewScroll = 0;

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
    this.theme = theme;
    this.allNodes = options.nodes;
    this.filter = options.initialFilter ?? "messages";
    this.query = options.initialQuery ?? "";
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

  private confirmSelected(summarize: boolean): void {
    const node = this.filtered[this.list.selected];
    if (!node) return;
    this.done({ action: "navigate", entryId: node.entry.id, summarize });
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
      const text = `${node.entry.roleLabel}: ${node.entry.summary}`;
      const highlighted = selected
        ? styleBold(th, th.fg(roleColor, text))
        : th.fg(roleColor, highlightTerms(text, this.query, highlightMatch));
      const row = `${pointer}${dot} ${th.fg("dim", node.prefix)}${labelTag}${highlighted}`;
      lines.push(pad(row));
    }
    return lines;
  }

  private buildRightLines(node: FlattenedTreeNode | undefined, rightWidth: number): string[] {
    const th = this.theme;
    const pad = (s: string) => fitWidth(s, rightWidth);
    if (!node) {
      return [pad(""), pad(th.fg("muted", "  Select a node to preview its content."))];
    }

    const highlightMatch = (m: string) => styleBold(th, th.fg("warning", m));
    const status = node.isLeaf
      ? th.fg("warning", "★ Current Leaf")
      : node.isOnActivePath
        ? th.fg("success", "● Active Branch")
        : th.fg("dim", "○ Alternate Branch");
    const lines: string[] = [
      pad(`  ${styleBold(th, th.fg("accent", node.entry.roleLabel))}  ${status}  ${th.fg("dim", node.entry.timestamp)}`),
      pad(`  ${th.fg("muted", "ID:")} ${th.fg("text", node.entry.id)}${node.entry.label ? `   ${th.fg("muted", "Label:")} ${th.fg("warning", node.entry.label)}` : ""}`),
      pad(th.fg("borderMuted", `  ${"─".repeat(Math.max(8, rightWidth - 4))}`)),
    ];

    const bodyW = Math.max(12, rightWidth - 4);
    for (const rawLine of node.entry.fullText.split(/\r?\n/)) {
      const wrapped = wrapTextToWidth(rawLine, bodyW);
      for (const seg of wrapped) {
        lines.push(pad(`  ${highlightTerms(seg, this.query, highlightMatch)}`));
      }
    }
    return lines;
  }

  render(width: number): string[] {
    const geom = computeSplitGeometry({
      width,
      rows: this.getRows(),
      leftRatio: 0.45,
      hasSearchBar: true,
      minLeftWidth: 28,
    });
    this.lastBodyHeight = geom.bodyHeight;
    this.lastBodyStartY = geom.bodyStartY;
    this.lastDividerX = geom.dividerX;
    this.list.clamp(this.filtered.length, geom.bodyHeight);

    const msgCount = this.allNodes.filter(
      (n) => n.entry.kind === "user" || n.entry.kind === "assistant" || n.entry.kind === "branch" || Boolean(n.entry.label),
    ).length;
    const labeledCount = this.allNodes.filter((n) => Boolean(n.entry.label)).length;

    const pills: HeaderPillSpec[] = [
      { id: "messages", label: `[Messages ${msgCount}]`, active: this.filter === "messages" },
      { id: "all", label: `[All ${this.allNodes.length}]`, active: this.filter === "all" },
      { id: "labeled", label: `[Labeled ${labeledCount}]`, active: this.filter === "labeled" },
    ];

    const leftLines = this.buildLeftLines(geom.leftWidth, geom.bodyHeight);
    const active = this.filtered[this.list.selected];
    const allPreview = this.buildRightLines(active, geom.rightWidth);
    const maxPreviewScroll = Math.max(0, allPreview.length - geom.bodyHeight);
    this.previewScroll = Math.max(0, Math.min(this.previewScroll, maxPreviewScroll));
    const rightLines = allPreview.slice(this.previewScroll, this.previewScroll + geom.bodyHeight);

    const frame = renderSplitOverlayFrame({
      geometry: geom,
      titlePrefix: ` ${styleBold(this.theme, this.theme.fg("accent", "/tree"))} `,
      pills,
      search: {
        promptPrefix: ` ${this.theme.fg("accent", "/")} `,
        query: this.query,
        placeholder: "search tree nodes, messages, or labels...",
        rightLabel: `(${this.filtered.length}/${this.allNodes.length})`,
      },
      leftLines,
      rightLines,
      footerHints: [
        { key: "Enter", label: "jump" },
        { key: "Ctrl+S", label: "jump+summary" },
        { key: "Tab", label: "filter" },
        { key: "Ctrl+U/D", label: "preview" },
        { key: "Esc", label: "close" },
      ],
      theme: this.theme,
    });
    this.lastPillRects = frame.pillRects;
    this.lastCloseRect = frame.closeRect;
    return frame.lines;
  }

  private handleMouse(data: string): boolean {
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
      } else if (this.list.move(delta, this.filtered.length, this.lastBodyHeight)) {
        this.previewScroll = 0;
      }
      this.tui.requestRender();
      return true;
    }

    if (!isPress || (btn & 3) !== 0) return true;

    if (this.lastCloseRect && hitRegion(this.lastCloseRect, col, row)) {
      this.done(undefined);
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
    if (this.handleMouse(data)) return;

    if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
      this.done(undefined);
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
    if (matchesKey(data, "tab")) {
      this.filter =
        this.filter === "messages" ? "all" : this.filter === "all" ? "labeled" : "messages";
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
    if (matchesKey(data, "pageUp")) {
      if (this.list.page(-1, this.filtered.length, this.lastBodyHeight)) this.previewScroll = 0;
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "pageDown")) {
      if (this.list.page(1, this.filtered.length, this.lastBodyHeight)) this.previewScroll = 0;
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "ctrl+u")) {
      this.previewScroll = Math.max(0, this.previewScroll - 5);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "ctrl+d")) {
      this.previewScroll += 5;
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
