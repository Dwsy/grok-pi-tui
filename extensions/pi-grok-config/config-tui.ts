/**
 * Bordered two-pane TS TUI renderer for `/pi-config` resource management.
 *
 * Pure UI layer over Rust's `PiResourceCatalog` JSON snapshot: every toggle or
 * project-override change invokes `onMutate(payload)` which Rust persists via
 * `PiResourceCatalog::apply_ui_mutation`.
 */

import path from "node:path";
import {
  matchesKey,
  parseKey,
  type Component,
  type TuiMouseEvent,
  type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import {
  computeSplitGeometry,
  detectNerdFontSupport,
  DividerDragController,
  fitWidth,
  getFileIcon,
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
  filterPiConfigResources,
  nextProjectOverride,
  readResourceFilePreview,
  type PiConfigMutationPayload,
  type PiConfigResource,
  type PiConfigSnapshot,
  type ResourceScopeView,
  type ResourceTypeFilter,
} from "./config-data.ts";

export interface PiConfigTuiOptions {
  snapshot: PiConfigSnapshot;
  initialScope?: ResourceScopeView;
  initialType?: ResourceTypeFilter;
  nerdIcons?: boolean;
  onMutate: (payload: PiConfigMutationPayload) => void;
  getRows?: () => number;
}

const TYPE_FILTERS: readonly ResourceTypeFilter[] = [
  "All",
  "Extensions",
  "Skills",
  "Prompts",
  "Themes",
];

const SGR_MOUSE_RE = /^\x1b\[<(\d+);(\d+);(\d+)([mM])$/;

class PiConfigTuiComponent implements Component {
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly snapshot: PiConfigSnapshot;
  private readonly onMutate: (payload: PiConfigMutationPayload) => void;
  private readonly nerdIcons: boolean;
  private readonly getRows: () => number;
  private readonly done: () => void;

  private scopeView: ResourceScopeView;
  private typeFilter: ResourceTypeFilter;
  private query = "";
  private searchActive = false;
  private filtered: PiConfigResource[] = [];
  private readonly list = new ListController({ rowsPerItem: 2 });
  private previewScroll = 0;
  private readonly previewCache = new Map<string, string[]>();
  private statusNotice = "";

  private lastTotalWidth = 100;
  private lastBodyHeight = 14;
  private lastBodyStartY = 5;
  private lastDividerX = 44;
  private lastPillRects = new Map<string, ClickRegion>();
  private lastCloseRect: ClickRegion | undefined;
  private readonly renderCoalescer: RenderCoalescer;
  private readonly dividerDrag = new DividerDragController(0.44);
  private rightLineCacheKey = "";
  private rightLineCache: string[] = [];

  constructor(
    tui: { requestRender: () => void },
    theme: TuiTheme,
    options: PiConfigTuiOptions,
    done: () => void,
  ) {
    this.tui = tui;
    this.theme = theme;
    // Clone resources array so optimistic UI updates stay isolated to this session.
    this.snapshot = {
      ...options.snapshot,
      resources: options.snapshot.resources.map((r) => ({ ...r })),
    };
    this.scopeView = options.initialScope ?? "Global";
    this.typeFilter = options.initialType ?? "All";
    this.nerdIcons = options.nerdIcons ?? detectNerdFontSupport();
    this.onMutate = options.onMutate;
    this.getRows = options.getRows ?? (() => 28);
    this.done = done;
    this.renderCoalescer = new RenderCoalescer(() => this.tui.requestRender());
    this.recomputeFilter(false);
  }

  private finish(): void {
    this.renderCoalescer.cancel();
    this.done();
  }

  invalidate(): void {
    // Recomputed on render().
  }

  private recomputeFilter(resetPreview = true): void {
    const prevId = this.filtered[this.list.selected]?.id;
    this.filtered = filterPiConfigResources(
      this.snapshot.resources,
      this.typeFilter,
      this.scopeView,
      this.query,
    );
    if (this.filtered.length === 0) {
      this.list.reset();
      this.previewScroll = 0;
      return;
    }
    const kept = prevId ? this.filtered.findIndex((r) => r.id === prevId) : -1;
    if (kept >= 0) {
      this.list.select(kept, this.filtered.length, this.lastBodyHeight);
    } else {
      this.list.clamp(this.filtered.length, this.lastBodyHeight);
      if (resetPreview) this.previewScroll = 0;
    }
  }

  private toggleSelected(): void {
    const resource = this.filtered[this.list.selected];
    if (!resource) return;

    if (this.scopeView === "Project") {
      if (!this.snapshot.projectTrusted) {
        this.statusNotice = "Project is untrusted — project overrides disabled";
        this.tui.requestRender();
        return;
      }
      const next = nextProjectOverride(resource.projectOverride);
      resource.projectOverride = next;
      this.statusNotice = `${resource.displayName}: project override → ${next} (reload to apply)`;
      this.onMutate({
        mode: "project",
        path: resource.path,
        resourceType: resource.resourceType,
        projectOverride: next,
      });
    } else {
      if (resource.scope !== "Global") {
        this.statusNotice = "Switch to [Project] scope to manage project-local resources";
        this.tui.requestRender();
        return;
      }
      const nextEnabled = !resource.enabled;
      resource.enabled = nextEnabled;
      resource.inheritedEnabled = nextEnabled;
      this.statusNotice = `${nextEnabled ? "Enabled" : "Disabled"} ${resource.displayName} globally (reload to apply)`;
      this.onMutate({
        mode: "global",
        path: resource.path,
        resourceType: resource.resourceType,
        enabled: nextEnabled,
      });
    }
    this.tui.requestRender();
  }

  private getFilePreview(filePath: string): string[] {
    let cached = this.previewCache.get(filePath);
    if (!cached) {
      cached = readResourceFilePreview(filePath);
      this.previewCache.set(filePath, cached);
    }
    return cached;
  }

  private buildLeftLines(leftWidth: number, bodyHeight: number): string[] {
    const th = this.theme;
    const pad = (s: string) => fitWidth(s, leftWidth);
    if (this.filtered.length === 0) {
      return [pad(""), pad(th.fg("muted", "  No matching Pi resources"))];
    }

    const highlightMatch = (m: string) => styleBold(th, th.fg("warning", m));
    const perPage = this.list.itemsPerPage(bodyHeight);
    const end = Math.min(this.filtered.length, this.list.scroll + perPage);
    const lines: string[] = [];

    for (let i = this.list.scroll; i < end; i++) {
      const r = this.filtered[i]!;
      const selected = i === this.list.selected;
      const pointer = selected ? th.fg("accent", "▸") : " ";
      const on = effectiveEnabled(r, this.scopeView);
      const check = on ? th.fg("success", "[✓]") : th.fg("dim", "[ ]");
      const iconSpec = getFileIcon(path.basename(r.path), { nerdIcons: this.nerdIcons });
      const icon = th.fg(iconSpec.color, iconSpec.glyph);
      const overrideTag =
        this.scopeView === "Project" && r.projectOverride !== "Inherit"
          ? ` ${th.fg(r.projectOverride === "Load" ? "success" : "error", r.projectOverride === "Load" ? "+Load" : "-Unload")}`
          : "";
      const nameStyled = selected
        ? styleBold(th, th.fg("accent", r.displayName))
        : th.fg("text", highlightTerms(r.displayName, this.query, highlightMatch));
      const row1 = `${pointer}${check} ${icon} ${nameStyled}${overrideTag}`;
      const row2 = `    ${th.fg("dim", `${r.resourceType} · ${r.origin} · ${r.source}`)}`;
      lines.push(pad(row1), pad(row2));
    }
    return lines;
  }

  private buildRightLines(resource: PiConfigResource | undefined, rightWidth: number): string[] {
    const th = this.theme;
    const pad = (s: string) => fitWidth(s, rightWidth);
    if (!resource) {
      return [
        pad(""),
        pad(th.fg("muted", "  Select a Pi resource to inspect or toggle.")),
      ];
    }

    const on = effectiveEnabled(resource, this.scopeView);
    const stateLabel = on ? th.fg("success", "Enabled") : th.fg("error", "Disabled");
    const trustLabel = this.snapshot.projectTrusted
      ? th.fg("success", "trusted")
      : th.fg("warning", "untrusted");

    const lines: string[] = [
      pad(`  ${styleBold(th, th.fg("accent", resource.displayName))}  ${stateLabel}`),
      pad(
        `  ${th.fg("muted", "Type:")} ${resource.resourceType}   ${th.fg("muted", "Scope:")} ${resource.scope}   ${th.fg("muted", "Origin:")} ${resource.origin}`,
      ),
      pad(
        `  ${th.fg("muted", "Override:")} ${resource.projectOverride}   ${th.fg("muted", "Project:")} ${trustLabel}`,
      ),
      pad(`  ${th.fg("muted", "Path:")} ${th.fg("dim", fitWidth(resource.path, Math.max(10, rightWidth - 9)))}`),
    ];
    if (this.statusNotice) {
      lines.push(pad(`  ${th.fg("warning", this.statusNotice)}`));
    }
    lines.push(pad(th.fg("borderMuted", `  ${"─".repeat(Math.max(8, rightWidth - 4))}`)));

    const bodyW = Math.max(12, rightWidth - 4);
    for (const raw of this.getFilePreview(resource.path)) {
      const wrapped = wrapTextToWidth(raw.replace(/\t/g, "  "), bodyW);
      for (const seg of wrapped) {
        lines.push(pad(`  ${th.fg("muted", seg)}`));
      }
    }
    return lines;
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
      ...TYPE_FILTERS.map((t) => ({
        id: `type:${t}`,
        label: `[${t}]`,
        active: this.typeFilter === t,
      })),
      {
        id: "scope:toggle",
        label: this.scopeView === "Global" ? "[Global]" : "[Project]",
        active: true,
        color: this.scopeView === "Project" ? "warning" : "success",
      },
    ];

    const leftLines = this.buildLeftLines(geom.leftWidth, geom.bodyHeight);
    const active = this.filtered[this.list.selected];
    const cacheKey = active
      ? `${active.id}:${geom.rightWidth}:${this.scopeView}:${active.enabled}:${active.projectOverride ?? ""}:${this.statusNotice}`
      : `empty:${geom.rightWidth}`;
    if (cacheKey !== this.rightLineCacheKey) {
      this.rightLineCacheKey = cacheKey;
      this.rightLineCache = this.buildRightLines(active, geom.rightWidth);
    }
    const allPreview = this.rightLineCache;
    const maxPreviewScroll = Math.max(0, allPreview.length - geom.bodyHeight);
    this.previewScroll = Math.max(0, Math.min(this.previewScroll, maxPreviewScroll));
    const rightLines = allPreview.slice(this.previewScroll, this.previewScroll + geom.bodyHeight);

    const frame = renderSplitOverlayFrame({
      geometry: geom,
      titlePrefix: ` ${styleBold(this.theme, this.theme.fg("accent", "/pi-config"))} `,
      pills,
      search: {
        promptPrefix: ` ${this.theme.fg(this.searchActive ? "accent" : "dim", "/")} `,
        query: this.query,
        placeholder: this.searchActive
          ? "type to filter (Enter keep, Esc clear)..."
          : "press / to search...",
        rightLabel: `(${this.filtered.length}/${this.snapshot.resources.length})`,
      },
      leftLines,
      rightLines,
      footerHints: [
        { key: "Tab", label: "Global/Project" },
        { key: "f", label: "type" },
        { key: "/", label: "search" },
        { key: "Space", label: this.scopeView === "Project" ? "override" : "toggle" },
        { key: "j/k", label: "move" },
        { key: "Esc", label: "close" },
      ],
      theme: this.theme,
    });
    this.lastPillRects = frame.pillRects;
    this.lastCloseRect = frame.closeRect;
    return frame.lines;
  }

  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    const dragRes = this.dividerDrag.handleMouse(event, {
      totalWidth: this.lastTotalWidth,
      dividerX: this.lastDividerX,
      bodyStartY: this.lastBodyStartY,
      bodyHeight: this.lastBodyHeight,
      minLeft: 28,
    });
    if (dragRes) return dragRes;

    const { x: col, y: row, type } = event;
    if (type === "wheel") {
      const delta = (event.wheelDelta ?? 0) > 0 ? 1 : -1;
      if (col > this.lastDividerX) {
        this.previewScroll = Math.max(0, this.previewScroll + delta * 3);
        this.renderCoalescer.schedule();
        return { handled: true, render: false };
      }
      if (this.list.move(delta, this.filtered.length, this.lastBodyHeight)) {
        this.previewScroll = 0;
      }
      return { handled: true, render: true };
    }
    if (type === "press" && event.button === "left") {
      if (this.lastCloseRect && hitRegion(this.lastCloseRect, col, row)) {
        this.finish();
        return { handled: true, render: false };
      }
      for (const [id, rect] of this.lastPillRects) {
        if (hitRegion(rect, col, row)) {
          if (id === "scope:toggle") {
            this.scopeView = this.scopeView === "Global" ? "Project" : "Global";
            this.recomputeFilter(true);
          } else if (id.startsWith("type:")) {
            const nextType = id.slice(5) as ResourceTypeFilter;
            if (nextType !== this.typeFilter) {
              this.typeFilter = nextType;
              this.recomputeFilter(true);
            }
          }
          return { handled: true, render: true };
        }
      }
      if (
        row >= this.lastBodyStartY &&
        row < this.lastBodyStartY + this.lastBodyHeight &&
        col >= 1 &&
        col < this.lastDividerX
      ) {
        const idx = this.list.indexAtBodyRow(row - this.lastBodyStartY, this.filtered.length);
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
        row >= this.lastBodyStartY &&
        row < this.lastBodyStartY + this.lastBodyHeight &&
        col >= 1 &&
        col < this.lastDividerX
      ) {
        const idx = this.list.indexAtBodyRow(row - this.lastBodyStartY, this.filtered.length);
        if (idx >= 0 && idx < this.filtered.length) {
          this.list.select(idx, this.filtered.length, this.lastBodyHeight);
          this.toggleSelected();
          return { handled: true, render: true };
        }
      }
    }
    return undefined;
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
        return true;
      } else if (this.list.move(delta, this.filtered.length, this.lastBodyHeight)) {
        this.previewScroll = 0;
      }
      this.tui.requestRender();
      return true;
    }

    if (!isPress || (btn & 3) !== 0) return true;

    if (this.lastCloseRect && hitRegion(this.lastCloseRect, col, row)) {
      this.finish();
      return true;
    }

    for (const [id, rect] of this.lastPillRects) {
      if (hitRegion(rect, col, row)) {
        if (id === "scope:toggle") {
          this.scopeView = this.scopeView === "Global" ? "Project" : "Global";
          this.recomputeFilter(true);
          this.tui.requestRender();
        } else if (id.startsWith("type:")) {
          const nextType = id.slice(5) as ResourceTypeFilter;
          if (nextType !== this.typeFilter) {
            this.typeFilter = nextType;
            this.recomputeFilter(true);
            this.tui.requestRender();
          }
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
        this.toggleSelected();
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

    if (matchesKey(data, "ctrl+c") || matchesKey(data, "f2")) {
      this.finish();
      return;
    }
    // Tab / Shift+Tab / Ctrl+G switches Global <-> Project (matching Rust pi_config.rs).
    if (matchesKey(data, "tab") || matchesKey(data, "shift+tab") || matchesKey(data, "ctrl+g")) {
      this.searchActive = false;
      this.scopeView = this.scopeView === "Global" ? "Project" : "Global";
      this.recomputeFilter(true);
      this.tui.requestRender();
      return;
    }

    if (this.searchActive) {
      if (matchesKey(data, "escape")) {
        this.searchActive = false;
        if (this.query.length > 0) {
          this.query = "";
          this.recomputeFilter(true);
        }
        this.tui.requestRender();
        return;
      }
      if (matchesKey(data, "enter") || matchesKey(data, "return")) {
        this.searchActive = false;
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
        return;
      }
      // Navigation keys exit search focus and fall through to list handlers.
      this.searchActive = false;
    }

    if (matchesKey(data, "escape")) {
      if (this.query.length > 0) {
        this.query = "";
        this.recomputeFilter(true);
        this.tui.requestRender();
        return;
      }
      this.finish();
      return;
    }
    if (data === "/") {
      this.searchActive = true;
      this.tui.requestRender();
      return;
    }
    if (data === "f") {
      const nextIdx = (TYPE_FILTERS.indexOf(this.typeFilter) + 1) % TYPE_FILTERS.length;
      this.typeFilter = TYPE_FILTERS[nextIdx]!;
      this.recomputeFilter(true);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "enter") || matchesKey(data, "return") || data === " ") {
      this.toggleSelected();
      return;
    }
    if (matchesKey(data, "up") || matchesKey(data, "ctrl+p") || data === "k") {
      if (this.list.move(-1, this.filtered.length, this.lastBodyHeight)) this.previewScroll = 0;
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "down") || matchesKey(data, "ctrl+n") || data === "j") {
      if (this.list.move(1, this.filtered.length, this.lastBodyHeight)) this.previewScroll = 0;
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "home") || data === "g") {
      if (this.list.select(0, this.filtered.length, this.lastBodyHeight)) this.previewScroll = 0;
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "end") || data === "G") {
      if (this.list.select(this.filtered.length - 1, this.filtered.length, this.lastBodyHeight)) {
        this.previewScroll = 0;
      }
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
  }
}

export function createPiConfigTui(
  tui: { requestRender: () => void },
  theme: TuiTheme,
  options: PiConfigTuiOptions,
  done: () => void,
): Component {
  return new PiConfigTuiComponent(tui, theme, options, done);
}
