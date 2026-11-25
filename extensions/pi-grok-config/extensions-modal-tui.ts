/**
 * Bordered two-pane TS TUI migration of Rust `extensions_modal.rs` for `grok-pi`
 * (`__pi_extensions_modal`).
 *
 * Reuses Rust's `PiResourceCatalog` JSON snapshot (and `pi_config_mutate` write-back)
 * plus live Pi MCP/tool discovery so Plugins, Skills, MCP Servers, Prompts/Workflows,
 * and Themes can be inspected, filtered, and toggled in one unified split TUI with
 * `DividerDragController`, `RenderCoalescer`, and bounded preview caching.
 */

import path from "node:path";
import {
  matchesKey,
  parseKey,
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
import {
  effectiveEnabled,
  nextProjectOverride,
  readResourceFilePreview,
  type PiConfigMutationPayload,
  type PiConfigResource,
  type PiConfigSnapshot,
  type ResourceScopeView,
} from "./config-data.ts";

export type ExtensionsModalTab = "plugins" | "skills" | "mcp" | "workflows" | "themes" | "all";
export type ExtensionsStatusFilter = "all" | "enabled" | "disabled";

export interface ExtensionsMcpItem {
  id: string;
  name: string;
  endpoint: string;
  transport: string;
  tools: Array<{ name: string; description: string }>;
}

export interface ExtensionsModalEntry {
  kind: "resource" | "mcp";
  id: string;
  tab: Exclude<ExtensionsModalTab, "all">;
  name: string;
  subtitle: string;
  enabled: boolean;
  searchLower: string;
  resource?: PiConfigResource;
  mcp?: ExtensionsMcpItem;
}

export interface ExtensionsModalTuiOptions {
  snapshot: PiConfigSnapshot;
  mcpServers?: readonly ExtensionsMcpItem[];
  initialTab?: ExtensionsModalTab;
  initialScope?: ResourceScopeView;
  onMutate: (payload: PiConfigMutationPayload) => void;
  getRows?: () => number;
}

const TAB_ORDER: readonly ExtensionsModalTab[] = [
  "plugins",
  "skills",
  "mcp",
  "workflows",
  "themes",
  "all",
];

const TAB_LABELS: Record<ExtensionsModalTab, string> = {
  plugins: "Plugins",
  skills: "Skills",
  mcp: "MCP",
  workflows: "Prompts",
  themes: "Themes",
  all: "All",
};

const STATUS_CYCLE: readonly ExtensionsStatusFilter[] = ["all", "enabled", "disabled"];

export function mapRustTabName(raw: string): ExtensionsModalTab {
  const norm = raw.trim().toLowerCase();
  if (norm === "skills") return "skills";
  if (norm === "mcp" || norm === "mcpservers" || norm === "mcp_servers") return "mcp";
  if (norm === "workflows" || norm === "prompts") return "workflows";
  if (norm === "themes") return "themes";
  if (norm === "all" || norm === "hooks" || norm === "marketplace") return "all";
  return "plugins";
}

function resourceTab(rt: PiConfigResource["resourceType"]): Exclude<ExtensionsModalTab, "all"> {
  if (rt === "Extensions") return "plugins";
  if (rt === "Skills") return "skills";
  if (rt === "Prompts") return "workflows";
  return "themes";
}

export function buildExtensionsEntries(
  snapshot: PiConfigSnapshot,
  scopeView: ResourceScopeView,
  mcpServers: readonly ExtensionsMcpItem[],
): ExtensionsModalEntry[] {
  const out: ExtensionsModalEntry[] = [];
  for (const r of snapshot.resources) {
    if (scopeView === "Global" && r.scope !== "Global") continue;
    const enabled = effectiveEnabled(r, scopeView);
    const tab = resourceTab(r.resourceType);
    out.push({
      kind: "resource",
      id: `res:${r.path}`,
      tab,
      name: r.displayName,
      subtitle: `${r.resourceType} · ${r.origin}${r.source ? ` · ${r.source}` : ""}`,
      enabled,
      searchLower: `${r.displayName} ${r.resourceType} ${r.origin} ${r.source} ${r.path}`.toLowerCase(),
      resource: r,
    });
  }
  for (const m of mcpServers) {
    out.push({
      kind: "mcp",
      id: `mcp:${m.id}`,
      tab: "mcp",
      name: m.name,
      subtitle: `MCP (${m.transport}) · ${m.tools.length} tool${m.tools.length === 1 ? "" : "s"}`,
      enabled: true,
      searchLower: `${m.name} ${m.endpoint} ${m.transport} ${m.tools.map((t) => t.name).join(" ")}`.toLowerCase(),
      mcp: m,
    });
  }
  return out;
}

export function filterExtensionsEntries(
  entries: readonly ExtensionsModalEntry[],
  tab: ExtensionsModalTab,
  status: ExtensionsStatusFilter,
  query: string,
): ExtensionsModalEntry[] {
  const terms = query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);
  return entries.filter((e) => {
    if (tab !== "all" && e.tab !== tab) return false;
    if (status === "enabled" && !e.enabled) return false;
    if (status === "disabled" && e.enabled) return false;
    if (terms.length === 0) return true;
    return terms.every((t) => e.searchLower.includes(t));
  });
}

export class ExtensionsModalTui implements Component {
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly snapshot: PiConfigSnapshot;
  private readonly mcpServers: readonly ExtensionsMcpItem[];
  private readonly onMutate: (payload: PiConfigMutationPayload) => void;
  private readonly getRows: () => number;
  private readonly done: () => void;

  private tab: ExtensionsModalTab;
  private scopeView: ResourceScopeView;
  private statusFilter: ExtensionsStatusFilter = "all";
  private query = "";
  private entries: ExtensionsModalEntry[] = [];
  private filtered: ExtensionsModalEntry[] = [];
  private readonly list = new ListController({ rowsPerItem: 2 });
  private previewScroll = 0;
  private statusNotice = "";
  private readonly filePreviewCache = new Map<string, string[]>();
  private rightCacheKey = "";
  private rightCacheLines: string[] = [];
  private readonly renderCoalescer: RenderCoalescer;
  private readonly dividerDrag = new DividerDragController(0.42);

  private lastTotalWidth = 100;
  private lastBodyHeight = 14;
  private lastBodyStartY = 5;
  private lastDividerX = 42;
  private lastPillRects = new Map<string, ClickRegion>();
  private lastCloseRect: ClickRegion | undefined;

  constructor(
    tui: { requestRender: () => void },
    theme: TuiTheme,
    options: ExtensionsModalTuiOptions,
    done: () => void,
  ) {
    this.tui = tui;
    this.theme = theme;
    this.snapshot = {
      ...options.snapshot,
      resources: options.snapshot.resources.map((r) => ({ ...r })),
    };
    this.mcpServers = options.mcpServers ?? [];
    this.tab = options.initialTab ?? "plugins";
    this.scopeView = options.initialScope ?? "Global";
    this.onMutate = options.onMutate;
    this.getRows = options.getRows ?? (() => 28);
    this.done = done;
    this.renderCoalescer = new RenderCoalescer(() => this.tui.requestRender());
    this.recompute(false);
  }

  getActiveTab(): ExtensionsModalTab {
    return this.tab;
  }

  getStatusFilter(): ExtensionsStatusFilter {
    return this.statusFilter;
  }

  getSelectedEntry(): ExtensionsModalEntry | undefined {
    return this.filtered[this.list.selected];
  }

  invalidate(): void {
    this.rightCacheKey = "";
  }

  private finish(): void {
    this.renderCoalescer.cancel();
    this.dividerDrag.cancel();
    this.done();
  }

  private recompute(resetPreview = true): void {
    const prevId = this.filtered[this.list.selected]?.id;
    this.entries = buildExtensionsEntries(this.snapshot, this.scopeView, this.mcpServers);
    this.filtered = filterExtensionsEntries(this.entries, this.tab, this.statusFilter, this.query);
    if (prevId) {
      const idx = this.filtered.findIndex((e) => e.id === prevId);
      if (idx >= 0) this.list.selected = idx;
    }
    this.list.clamp(this.filtered.length, this.lastBodyHeight);
    if (resetPreview) this.previewScroll = 0;
    this.rightCacheKey = "";
  }

  private toggleSelected(): void {
    const entry = this.filtered[this.list.selected];
    if (!entry) return;
    if (entry.kind !== "resource" || !entry.resource) {
      this.statusNotice = "MCP servers are managed in Pi settings / MCP config";
      this.tui.requestRender();
      return;
    }
    const item = entry.resource;
    if (this.scopeView === "Project") {
      if (!this.snapshot.projectTrusted) {
        this.statusNotice = "Project directory is untrusted — project overrides blocked";
        this.tui.requestRender();
        return;
      }
      const next = nextProjectOverride(item.projectOverride);
      item.projectOverride = next;
      this.onMutate({
        mode: "project",
        path: item.path,
        resourceType: item.resourceType,
        projectOverride: next,
      });
      this.statusNotice = `${item.displayName} → project ${next}`;
    } else {
      const nextEnabled = !item.enabled;
      item.enabled = nextEnabled;
      this.onMutate({
        mode: "global",
        path: item.path,
        resourceType: item.resourceType,
        enabled: nextEnabled,
      });
      this.statusNotice = `${item.displayName} → ${nextEnabled ? "enabled" : "disabled"}`;
    }
    this.recompute(false);
    this.tui.requestRender();
  }

  private renderLeftPane(leftWidth: number, height: number): string[] {
    const th = this.theme;
    this.list.clamp(this.filtered.length, height);
    if (this.filtered.length === 0) {
      const empty = [
        "",
        th.fg("dim", `  No ${TAB_LABELS[this.tab].toLowerCase()} matched`),
        th.fg("dim", "  Press Tab to switch category or Ctrl+F for status filter"),
      ];
      while (empty.length < height) empty.push("");
      return empty.slice(0, height).map((l) => fitWidth(l, leftWidth));
    }

    const lines: string[] = [];
    const perPage = this.list.itemsPerPage(height);
    const hl = (m: string) => `\x1b[7m${m}\x1b[27m`;
    for (let slot = 0; slot < perPage; slot++) {
      const idx = this.list.scroll + slot;
      if (idx >= this.filtered.length) break;
      const entry = this.filtered[idx]!;
      const selected = idx === this.list.selected;
      const marker = selected ? th.fg("accent", "▸ ") : "  ";
      const badge = entry.enabled ? th.fg("success", "● ") : th.fg("dim", "○ ");
      const budget = Math.max(6, leftWidth - 4);
      const nameTrunc = truncateToWidth(entry.name, budget, "…");
      const nameStyled = selected
        ? styleBold(th, th.fg("accent", nameTrunc))
        : this.query.trim()
          ? highlightTerms(nameTrunc, this.query, hl)
          : nameTrunc;
      const row1 = `${marker}${badge}${nameStyled}`;
      const subText = truncateToWidth(entry.subtitle, Math.max(4, leftWidth - 4), "…");
      const row2 = `    ${th.fg("dim", this.query.trim() ? highlightTerms(subText, this.query, hl) : subText)}`;
      lines.push(fitWidth(row1, leftWidth));
      if (lines.length < height) lines.push(fitWidth(row2, leftWidth));
    }
    while (lines.length < height) lines.push(" ".repeat(leftWidth));
    return lines.slice(0, height);
  }

  private renderRightPane(rightWidth: number, height: number): string[] {
    const th = this.theme;
    const selected = this.filtered[this.list.selected];
    if (!selected) {
      const empty = ["", th.fg("dim", "  Select an item to inspect details")];
      while (empty.length < height) empty.push("");
      return empty.slice(0, height).map((l) => fitWidth(l, rightWidth));
    }

    const cacheKey = `${selected.id}:${selected.enabled}:${this.scopeView}:${rightWidth}:${this.statusNotice}`;
    if (cacheKey !== this.rightCacheKey) {
      this.rightCacheKey = cacheKey;
      const out: string[] = [];
      const textW = Math.max(12, rightWidth - 2);
      out.push(` ${styleBold(th, th.fg("accent", selected.name))}`);
      out.push(
        ` ${th.fg("dim", "Status:")} ${selected.enabled ? th.fg("success", "enabled") : th.fg("warning", "disabled")}  ${th.fg("dim", "Category:")} ${TAB_LABELS[selected.tab]}`,
      );
      if (selected.kind === "resource" && selected.resource) {
        const r = selected.resource;
        out.push(
          ` ${th.fg("dim", "Scope:")} ${r.scope}  ${th.fg("dim", "Origin:")} ${r.origin}${r.source ? `  ${th.fg("dim", "Source:")} ${r.source}` : ""}`,
        );
        if (this.scopeView === "Project") {
          out.push(
            ` ${th.fg("dim", "Project override:")} ${th.fg("accent", r.projectOverride)}  ${th.fg("dim", "Trusted:")} ${this.snapshot.projectTrusted ? th.fg("success", "yes") : th.fg("warning", "no")}`,
          );
        }
        out.push(` ${th.fg("dim", "Path:")} ${truncateToWidth(r.path, Math.max(8, textW - 6), "…")}`);
        if (this.statusNotice) {
          out.push(` ${th.fg("success", `✓ ${this.statusNotice}`)}`);
        }
        out.push(th.fg("border", "─".repeat(rightWidth)));
        let preview = this.filePreviewCache.get(r.path);
        if (!preview) {
          if (this.filePreviewCache.size >= 16) this.filePreviewCache.clear();
          preview = readResourceFilePreview(r.path);
          this.filePreviewCache.set(r.path, preview);
        }
        const baseName = path.basename(r.path);
        out.push(` ${th.fg("dim", baseName)}`);
        for (const line of preview) {
          for (const wrapped of wrapTextToWidth(line, textW)) {
            out.push(` ${wrapped}`);
          }
        }
      } else if (selected.mcp) {
        const m = selected.mcp;
        out.push(` ${th.fg("dim", "Transport:")} ${m.transport}`);
        out.push(` ${th.fg("dim", "Endpoint:")} ${truncateToWidth(m.endpoint || "(local)", Math.max(8, textW - 10), "…")}`);
        if (this.statusNotice) {
          out.push(` ${th.fg("warning", this.statusNotice)}`);
        }
        out.push(th.fg("border", "─".repeat(rightWidth)));
        out.push(` ${styleBold(th, `Tools (${m.tools.length})`)}`);
        if (m.tools.length === 0) {
          out.push(th.fg("dim", "  (no registered tools)"));
        } else {
          for (const tool of m.tools) {
            out.push(` ${th.fg("accent", "•")} ${styleBold(th, tool.name)}`);
            if (tool.description) {
              for (const w of wrapTextToWidth(tool.description, Math.max(10, textW - 3))) {
                out.push(`   ${th.fg("dim", w)}`);
              }
            }
          }
        }
      }
      this.rightCacheLines = out;
    }

    const maxScroll = Math.max(0, this.rightCacheLines.length - height);
    if (this.previewScroll > maxScroll) this.previewScroll = maxScroll;
    const visible = this.rightCacheLines.slice(this.previewScroll, this.previewScroll + height);
    while (visible.length < height) visible.push("");
    return visible.map((l) => fitWidth(l, rightWidth));
  }

  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    const dragRes = this.dividerDrag.handleMouse(event, {
      totalWidth: this.lastTotalWidth,
      dividerX: this.lastDividerX,
      bodyStartY: this.lastBodyStartY,
      bodyHeight: this.lastBodyHeight,
      minLeft: 26,
      minRight: 28,
    });
    if (dragRes) return dragRes;

    const { x, y, type } = event;
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
        this.finish();
        return { handled: true, render: false };
      }
      for (const [id, rect] of this.lastPillRects) {
        if (!hitRegion(rect, x, y)) continue;
        if (id === "scope:toggle") {
          this.scopeView = this.scopeView === "Global" ? "Project" : "Global";
          this.recompute(true);
          return { handled: true, render: true };
        }
        if (id === "status:cycle") {
          const next = STATUS_CYCLE[(STATUS_CYCLE.indexOf(this.statusFilter) + 1) % STATUS_CYCLE.length]!;
          this.statusFilter = next;
          this.recompute(true);
          return { handled: true, render: true };
        }
        if (id.startsWith("tab:")) {
          this.tab = id.slice(4) as ExtensionsModalTab;
          this.recompute(true);
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
          this.toggleSelected();
          return { handled: true, render: true };
        }
      }
    }
    return undefined;
  }

  handleInput(data: string): void {
    if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
      this.finish();
      return;
    }
    if (matchesKey(data, "tab")) {
      const idx = TAB_ORDER.indexOf(this.tab);
      this.tab = TAB_ORDER[(idx + 1) % TAB_ORDER.length]!;
      this.recompute(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "shift+tab")) {
      const idx = TAB_ORDER.indexOf(this.tab);
      this.tab = TAB_ORDER[(idx - 1 + TAB_ORDER.length) % TAB_ORDER.length]!;
      this.recompute(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "ctrl+g")) {
      this.scopeView = this.scopeView === "Global" ? "Project" : "Global";
      this.recompute(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "ctrl+f")) {
      this.statusFilter =
        STATUS_CYCLE[(STATUS_CYCLE.indexOf(this.statusFilter) + 1) % STATUS_CYCLE.length]!;
      this.recompute(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "enter") || matchesKey(data, "return") || data === " ") {
      this.toggleSelected();
      return;
    }
    if (matchesKey(data, "up")) {
      if (this.list.move(-1, this.filtered.length, this.lastBodyHeight)) {
        this.previewScroll = 0;
        this.tui.requestRender();
      }
      return;
    }
    if (matchesKey(data, "down")) {
      if (this.list.move(1, this.filtered.length, this.lastBodyHeight)) {
        this.previewScroll = 0;
        this.tui.requestRender();
      }
      return;
    }
    if (matchesKey(data, "pageUp") || matchesKey(data, "ctrl+u")) {
      this.previewScroll = Math.max(0, this.previewScroll - Math.max(4, Math.floor(this.lastBodyHeight / 2)));
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "pageDown") || matchesKey(data, "ctrl+d")) {
      this.previewScroll += Math.max(4, Math.floor(this.lastBodyHeight / 2));
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "backspace")) {
      if (this.query.length > 0) {
        this.query = Array.from(this.query).slice(0, -1).join("");
        this.recompute(true);
        this.tui.requestRender();
      }
      return;
    }
    if (matchesKey(data, "ctrl+w") || matchesKey(data, "ctrl+l")) {
      if (this.query.length > 0) {
        this.query = "";
        this.recompute(true);
        this.tui.requestRender();
      }
      return;
    }

    const parsed = parseKey(data);
    if (parsed && parsed.length === 1 && !/[\x00-\x1f\x7f]/.test(parsed)) {
      this.query += parsed;
      this.recompute(true);
      this.tui.requestRender();
    }
  }

  render(width: number): string[] {
    const termRows = this.getRows();
    const targetRows = Math.max(16, Math.floor(termRows * 0.9));
    const geom = computeSplitGeometry({
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
    this.lastTotalWidth = geom.totalWidth;
    this.lastBodyHeight = geom.bodyHeight;
    this.lastBodyStartY = geom.bodyStartY;
    this.lastDividerX = geom.dividerX;

    const pills: HeaderPillSpec[] = [
      ...TAB_ORDER.map((t) => ({
        id: `tab:${t}`,
        label: `[${TAB_LABELS[t]}]`,
        active: this.tab === t,
      })),
      {
        id: "status:cycle",
        label: `[${this.statusFilter}]`,
        active: this.statusFilter !== "all",
        color: this.statusFilter === "disabled" ? "warning" : "accent",
      },
      {
        id: "scope:toggle",
        label: `[${this.scopeView}]`,
        active: true,
        color: this.scopeView === "Project" ? "warning" : "success",
      },
    ];

    const frame = renderSplitOverlayFrame({
      geometry: geom,
      titlePrefix: ` ${styleBold(this.theme, this.theme.fg("accent", "Extensions"))} `,
      pills,
      search: {
        promptPrefix: ` ${this.theme.fg("accent", "Search:")} `,
        query: this.query,
        placeholder: "filter plugins, skills, mcp, prompts, themes...",
        rightLabel: `${this.filtered.length}/${this.entries.length} `,
      },
      leftLines: this.renderLeftPane(geom.leftWidth, geom.bodyHeight),
      rightLines: this.renderRightPane(geom.rightWidth, geom.bodyHeight),
      footerHints: [
        { key: "Tab", label: "category" },
        { key: "Space/Enter", label: "toggle" },
        { key: "Ctrl+F", label: "status" },
        { key: "Ctrl+G", label: "scope" },
        { key: "Drag │", label: "resize" },
        { key: "Esc", label: "close" },
      ],
      theme: this.theme,
    });
    this.lastPillRects = frame.pillRects;
    this.lastCloseRect = frame.closeRect;
    return frame.lines.map((l) => (visibleWidth(l) === geom.totalWidth ? l : fitWidth(l, geom.totalWidth)));
  }
}

export function createExtensionsModalTui(
  tui: { requestRender: () => void },
  theme: TuiTheme,
  options: ExtensionsModalTuiOptions,
  done: () => void,
): ExtensionsModalTui {
  return new ExtensionsModalTui(tui, theme, options, done);
}
