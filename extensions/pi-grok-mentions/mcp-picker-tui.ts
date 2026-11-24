/**
 * Bordered two-pane TS TUI overlay for `@mcp` mentions.
 *
 * Built on `../pi-grok-tui-kit/` so its frame, pills, search bar, mouse hit-testing,
 * and CJK-safe width handling match `@session`, `@skill`, `@file`, and `/review-*`.
 *
 * Left pane:  Pi-registered MCP servers with tool counts & endpoint/command preview.
 * Right pane: Selected server metadata + scrollable list of tools and descriptions.
 */

import { matchesKey, parseKey, type Component } from "@earendil-works/pi-tui";
import {
  computeSplitGeometry,
  fitWidth,
  highlightTerms,
  hitRegion,
  renderSplitOverlayFrame,
  styleBold,
  wrapTextToWidth,
  type ClickRegion,
  type HeaderPillSpec,
  type TuiTheme,
} from "../pi-grok-tui-kit/index.ts";

export interface McpToolRecord {
  name: string;
  fullName: string;
  description: string;
}

export interface McpServerRecord {
  id: string;
  name: string;
  endpoint: string;
  transport: "http" | "stdio" | "tools";
  tools: McpToolRecord[];
  searchText: string;
}

export type McpFilterMode = "all" | "with-tools";

export interface McpPickerOptions {
  records: McpServerRecord[];
  initialQuery?: string;
  initialFilter?: McpFilterMode;
  getRows?: () => number;
}

const SGR_MOUSE_RE = /^\x1b\[<(\d+);(\d+);(\d+)([mM])$/;
const DOUBLE_CLICK_MS = 450;

export function filterMcpServers(
  records: McpServerRecord[],
  query: string,
  filter: McpFilterMode = "all",
): McpServerRecord[] {
  const scoped =
    filter === "with-tools" ? records.filter((r) => r.tools.length > 0) : records;
  const terms = query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);
  if (terms.length === 0) return scoped;

  const scored: Array<{ record: McpServerRecord; score: number }> = [];
  for (const record of scoped) {
    const hay = record.searchText.toLowerCase();
    if (!terms.every((t) => hay.includes(t))) continue;

    const nameLower = record.name.toLowerCase();
    let score = 0;
    for (const t of terms) {
      if (nameLower === t) score += 100;
      else if (nameLower.startsWith(t)) score += 60;
      else if (nameLower.includes(t)) score += 35;
      else if (record.tools.some((tool) => tool.name.toLowerCase().includes(t))) score += 20;
      else score += 8;
    }
    scored.push({ record, score });
  }

  scored.sort(
    (a, b) =>
      b.score - a.score ||
      b.record.tools.length - a.record.tools.length ||
      a.record.name.localeCompare(b.record.name),
  );
  return scored.map((s) => s.record);
}

class McpPickerComponent implements Component {
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly allRecords: McpServerRecord[];
  private readonly getRows: () => number;
  private readonly done: (result: McpServerRecord | undefined) => void;

  private filter: McpFilterMode;
  private query: string;
  private filtered: McpServerRecord[] = [];
  private selectedIndex = 0;
  private listScroll = 0;
  private previewScroll = 0;

  private lastBodyHeight = 14;
  private lastBodyStartY = 5;
  private lastDividerX = 37;
  private lastPillRects = new Map<string, ClickRegion>();
  private lastCloseRect: ClickRegion | undefined;
  private lastClickIndex = -1;
  private lastClickTime = 0;

  constructor(
    tui: { requestRender: () => void },
    theme: TuiTheme,
    options: McpPickerOptions,
    done: (result: McpServerRecord | undefined) => void,
  ) {
    this.tui = tui;
    this.theme = theme;
    this.allRecords = options.records;
    this.filter = options.initialFilter ?? "all";
    this.query = options.initialQuery ?? "";
    this.getRows = options.getRows ?? (() => 28);
    this.done = done;
    this.recomputeFilter(false);
  }

  invalidate(): void {
    // Recomputed on render().
  }

  private recomputeFilter(resetPreview = true): void {
    const prevId = this.filtered[this.selectedIndex]?.id;
    this.filtered = filterMcpServers(this.allRecords, this.query, this.filter);
    if (this.filtered.length === 0) {
      this.selectedIndex = 0;
      this.listScroll = 0;
      this.previewScroll = 0;
      return;
    }
    const kept = prevId ? this.filtered.findIndex((r) => r.id === prevId) : -1;
    if (kept >= 0) {
      this.selectedIndex = kept;
    } else {
      this.selectedIndex = Math.min(this.selectedIndex, this.filtered.length - 1);
      if (resetPreview) this.previewScroll = 0;
    }
    this.ensureListVisible();
  }

  private get itemsPerPage(): number {
    return Math.max(1, Math.floor(this.lastBodyHeight / 2));
  }

  private ensureListVisible(): void {
    const perPage = this.itemsPerPage;
    if (this.selectedIndex < this.listScroll) {
      this.listScroll = this.selectedIndex;
    } else if (this.selectedIndex >= this.listScroll + perPage) {
      this.listScroll = this.selectedIndex - perPage + 1;
    }
    const maxScroll = Math.max(0, this.filtered.length - perPage);
    this.listScroll = Math.max(0, Math.min(this.listScroll, maxScroll));
  }

  private selectIndex(next: number): void {
    if (this.filtered.length === 0) return;
    const clamped = Math.max(0, Math.min(this.filtered.length - 1, next));
    if (clamped !== this.selectedIndex) {
      this.selectedIndex = clamped;
      this.previewScroll = 0;
      this.ensureListVisible();
    }
  }

  private confirmSelected(): void {
    const chosen = this.filtered[this.selectedIndex];
    if (chosen) this.done(chosen);
  }

  private buildPreviewLines(record: McpServerRecord | undefined, rightWidth: number): string[] {
    const th = this.theme;
    const pad = (s: string) => fitWidth(s, rightWidth);

    if (!record) {
      return [
        pad(""),
        pad(th.fg("muted", "  No MCP servers match the current filter.")),
        pad(th.fg("dim", "  Check your Pi MCP config or clear the search query.")),
      ];
    }

    const highlightMatch = (m: string) => styleBold(th, th.fg("warning", m));
    const lines: string[] = [];

    lines.push(pad(styleBold(th, th.fg("accent", `  @mcp:${record.name}`))));
    lines.push(
      pad(
        `  ${th.fg("muted", "Transport:")} ${th.fg("text", record.transport)}   ${th.fg("muted", "Tools:")} ${th.fg("accent", String(record.tools.length))}`,
      ),
    );
    lines.push(
      pad(
        `  ${th.fg("muted", "Endpoint:")}  ${th.fg("text", fitWidth(record.endpoint || "MCP server", Math.max(12, rightWidth - 14)))}`,
      ),
    );
    lines.push(pad(th.fg("borderMuted", `  ${"─".repeat(Math.max(8, rightWidth - 4))}`)));

    if (record.tools.length === 0) {
      lines.push(pad(th.fg("dim", "  No individual tools reported by this server yet.")));
      return lines;
    }

    const descWidth = Math.max(12, rightWidth - 6);
    for (const tool of record.tools) {
      const toolLabel = th.fg("accent", highlightTerms(tool.name, this.query, highlightMatch));
      lines.push(pad(`  ${th.fg("accent", "•")} ${toolLabel}`));
      if (tool.description) {
        const wrapped = wrapTextToWidth(tool.description.replace(/\s+/g, " ").trim(), descWidth);
        for (const seg of wrapped.slice(0, 3)) {
          lines.push(pad(`    ${th.fg("muted", highlightTerms(seg, this.query, highlightMatch))}`));
        }
      }
    }

    return lines;
  }

  private buildLeftLines(leftWidth: number, bodyHeight: number): string[] {
    const th = this.theme;
    const pad = (s: string) => fitWidth(s, leftWidth);
    const lines: string[] = [];
    if (this.filtered.length === 0) {
      lines.push(pad(""));
      lines.push(pad(th.fg("muted", "  No matching MCP servers")));
      return lines;
    }

    const highlightMatch = (m: string) => styleBold(th, th.fg("warning", m));
    const perPage = Math.max(1, Math.floor(bodyHeight / 2));
    const end = Math.min(this.filtered.length, this.listScroll + perPage);

    for (let i = this.listScroll; i < end; i++) {
      const rec = this.filtered[i]!;
      const selected = i === this.selectedIndex;
      const badge = `${rec.tools.length} tools`;
      const pointer = selected ? th.fg("accent", "▸ ") : "  ";
      const nameMax = Math.max(8, leftWidth - 4 - badge.length);
      const nameStr = fitWidth(rec.name, nameMax);
      const styledName = selected
        ? styleBold(th, th.fg("accent", nameStr))
        : th.fg("text", highlightTerms(nameStr, this.query, highlightMatch));
      const badgeStyled = th.fg(selected ? "accent" : "dim", badge);
      const row1 = pad(`${pointer}${styledName} ${badgeStyled}`);

      const subMax = Math.max(8, leftWidth - 3);
      const subRaw = fitWidth(`${rec.transport} · ${rec.endpoint}`, subMax);
      const row2 = pad(`  ${th.fg("muted", highlightTerms(subRaw, this.query, highlightMatch))}`);

      lines.push(row1, row2);
    }

    return lines;
  }

  render(width: number): string[] {
    const termRows = this.getRows();
    const targetRows = Math.max(16, Math.floor(termRows * 0.9));
    const geom = computeSplitGeometry({
      width,
      rows: termRows,
      leftRatio: 0.38,
      hasSearchBar: true,
      minLeftWidth: 26,
      heightRatio: 0.9,
      maxRows: targetRows,
    });
    this.lastBodyHeight = geom.bodyHeight;
    this.lastBodyStartY = geom.bodyStartY;
    this.lastDividerX = geom.dividerX;
    this.ensureListVisible();

    const withToolsCount = this.allRecords.filter((r) => r.tools.length > 0).length;
    const pills: HeaderPillSpec[] = [
      { id: "all", label: `[All ${this.allRecords.length}]`, active: this.filter === "all" },
      {
        id: "with-tools",
        label: `[With Tools ${withToolsCount}]`,
        active: this.filter === "with-tools",
      },
    ];

    const leftLines = this.buildLeftLines(geom.leftWidth, geom.bodyHeight);
    const active = this.filtered[this.selectedIndex];
    const allPreview = this.buildPreviewLines(active, geom.rightWidth);
    const maxPreviewScroll = Math.max(0, allPreview.length - geom.bodyHeight);
    this.previewScroll = Math.max(0, Math.min(this.previewScroll, maxPreviewScroll));
    const rightLines = allPreview.slice(this.previewScroll, this.previewScroll + geom.bodyHeight);

    const titlePrefix = ` ${styleBold(this.theme, this.theme.fg("accent", "@mcp"))} `;
    const rightLabel = `(${this.filtered.length}/${this.allRecords.length})`;

    const frame = renderSplitOverlayFrame({
      geometry: geom,
      titlePrefix,
      pills,
      search: {
        promptPrefix: ` ${this.theme.fg("accent", "/")} `,
        query: this.query,
        placeholder: "filter MCP servers or tools...",
        rightLabel,
      },
      leftLines,
      rightLines,
      footerHints: [
        { key: "Enter", label: "insert" },
        { key: "Tab", label: "filter" },
        { key: "↑/↓", label: "select" },
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
      } else {
        this.selectIndex(this.selectedIndex + delta);
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
        const nextFilter = id as McpFilterMode;
        if (nextFilter !== this.filter) {
          this.filter = nextFilter;
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
      const relRow = row - this.lastBodyStartY;
      const idx = this.listScroll + Math.floor(relRow / 2);
      if (idx >= 0 && idx < this.filtered.length) {
        const now = Date.now();
        if (idx === this.lastClickIndex && now - this.lastClickTime <= DOUBLE_CLICK_MS) {
          this.selectedIndex = idx;
          this.confirmSelected();
          return true;
        }
        this.lastClickIndex = idx;
        this.lastClickTime = now;
        this.selectIndex(idx);
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
      this.confirmSelected();
      return;
    }
    if (matchesKey(data, "tab")) {
      this.filter = this.filter === "all" ? "with-tools" : "all";
      this.recomputeFilter(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "up") || matchesKey(data, "ctrl+p")) {
      this.selectIndex(this.selectedIndex - 1);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "down") || matchesKey(data, "ctrl+n")) {
      this.selectIndex(this.selectedIndex + 1);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "pageUp")) {
      this.selectIndex(this.selectedIndex - this.itemsPerPage);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "pageDown")) {
      this.selectIndex(this.selectedIndex + this.itemsPerPage);
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

export function createMcpPickerTui(
  tui: { requestRender: () => void },
  theme: TuiTheme,
  options: McpPickerOptions,
  done: (result: McpServerRecord | undefined) => void,
): Component {
  return new McpPickerComponent(tui, theme, options, done);
}
