/**
 * Bordered two-pane TS TUI migration of Rust `goal_detail.rs` for `grok-pi`
 * (`__pi_goal_detail` / `/goal-detail`).
 *
 * Displays the active goal's objective, status/phase, token usage, pause/block
 * reason, todo progress (if any), and chronological goal transition history
 * reconstructed from Pi's session journal (`pi-grok-goal/v1`).
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

export interface GoalDetailSnapshot {
  goalId: string;
  objective: string;
  status: string;
  phase: string;
  tokenBudget?: number;
  tokensUsed: number;
  createdAt: string;
  pauseMessage?: string;
  lastEvent?: string;
  lastEventDetail?: string;
}

export interface GoalHistoryEvent {
  timestamp: string;
  event: string;
  status: string;
  detail: string;
}

export interface GoalDetailTuiOptions {
  goal: GoalDetailSnapshot;
  history: readonly GoalHistoryEvent[];
  getRows?: () => number;
}

interface GoalSectionItem {
  id: string;
  title: string;
  subtitle: string;
  badgeColor: string;
  bodyLines: string[];
}

function formatCompactTokens(tokens: number): string {
  if (!Number.isFinite(tokens) || tokens <= 0) return "0";
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(2)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}k`;
  return String(Math.round(tokens));
}

function statusColor(status: string): string {
  if (status === "active" || status === "complete") return "success";
  if (status === "user_paused" || status === "blocked") return "warning";
  return "error";
}

function buildSections(
  goal: GoalDetailSnapshot,
  history: readonly GoalHistoryEvent[],
): GoalSectionItem[] {
  const sections: GoalSectionItem[] = [
    {
      id: "overview",
      title: "Objective & Status",
      subtitle: `${goal.status} · ${goal.phase || "idle"} · ${formatCompactTokens(goal.tokensUsed)} tok`,
      badgeColor: statusColor(goal.status),
      bodyLines: [
        `Status: ${goal.status} (phase: ${goal.phase || "idle"})`,
        `Goal ID: ${goal.goalId || "(none)"}`,
        `Created: ${goal.createdAt || "(unknown)"}`,
        `Tokens used: ${formatCompactTokens(goal.tokensUsed)}${goal.tokenBudget ? ` / ${formatCompactTokens(goal.tokenBudget)}` : ""}`,
        ...(goal.pauseMessage ? ["", "Pause / Block Reason:", goal.pauseMessage] : []),
        "",
        "Objective:",
        goal.objective || "(no objective)",
      ],
    },
  ];

  const ordered = [...history].reverse();
  for (let i = 0; i < ordered.length; i++) {
    const h = ordered[i]!;
    sections.push({
      id: `hist-${i}`,
      title: h.event || h.status || "transition",
      subtitle: `${h.timestamp ? h.timestamp.slice(11, 19) : "—"} · ${h.status}`,
      badgeColor: statusColor(h.status),
      bodyLines: [
        `Event: ${h.event || "(update)"}`,
        `Status: ${h.status}`,
        `Timestamp: ${h.timestamp || "(unknown)"}`,
        "",
        h.detail || "(no details recorded)",
      ],
    });
  }
  return sections;
}

export class GoalDetailTui implements Component {
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly goal: GoalDetailSnapshot;
  private readonly sections: GoalSectionItem[];
  private readonly getRows: () => number;
  private readonly done: () => void;

  private readonly list = new ListController({ rowsPerItem: 2 });
  private previewScroll = 0;
  private previewCacheKey = "";
  private previewCacheLines: string[] = [];
  private readonly renderCoalescer: RenderCoalescer;
  private readonly dividerDrag = new DividerDragController(0.36);

  private lastTotalWidth = 100;
  private lastBodyHeight = 14;
  private lastBodyStartY = 3;
  private lastDividerX = 36;
  private lastCloseRect: ClickRegion | undefined;

  constructor(
    tui: { requestRender: () => void },
    theme: TuiTheme,
    options: GoalDetailTuiOptions,
    done: () => void,
  ) {
    this.tui = tui;
    this.theme = theme;
    this.goal = options.goal;
    this.sections = buildSections(options.goal, options.history);
    this.getRows = options.getRows ?? (() => 28);
    this.done = done;
    this.renderCoalescer = new RenderCoalescer(() => this.tui.requestRender());
  }

  getSelectedSectionId(): string | undefined {
    return this.sections[this.list.selected]?.id;
  }

  invalidate(): void {
    this.previewCacheKey = "";
  }

  private finish(): void {
    this.renderCoalescer.cancel();
    this.dividerDrag.cancel();
    this.done();
  }

  private renderLeftPane(leftWidth: number, height: number): string[] {
    const th = this.theme;
    this.list.clamp(this.sections.length, height);
    const lines: string[] = [];
    const perPage = this.list.itemsPerPage(height);
    for (let slot = 0; slot < perPage; slot++) {
      const idx = this.list.scroll + slot;
      if (idx >= this.sections.length) break;
      const item = this.sections[idx]!;
      const selected = idx === this.list.selected;
      const marker = selected ? th.fg("accent", "▸ ") : "  ";
      const dot = th.fg(item.badgeColor, "● ");
      const title = truncateToWidth(item.title, Math.max(6, leftWidth - 4), "…");
      const row1 = `${marker}${dot}${selected ? styleBold(th, th.fg("accent", title)) : title}`;
      const row2 = `    ${th.fg("dim", truncateToWidth(item.subtitle, Math.max(4, leftWidth - 4), "…"))}`;
      lines.push(fitWidth(row1, leftWidth));
      if (lines.length < height) lines.push(fitWidth(row2, leftWidth));
    }
    while (lines.length < height) lines.push(" ".repeat(leftWidth));
    return lines.slice(0, height);
  }

  private renderRightPane(rightWidth: number, height: number): string[] {
    const th = this.theme;
    const item = this.sections[this.list.selected];
    if (!item) {
      const empty = ["", th.fg("dim", "  No goal details available")];
      while (empty.length < height) empty.push("");
      return empty.slice(0, height).map((l) => fitWidth(l, rightWidth));
    }

    const cacheKey = `${item.id}:${rightWidth}`;
    if (cacheKey !== this.previewCacheKey) {
      this.previewCacheKey = cacheKey;
      const textW = Math.max(12, rightWidth - 2);
      const out: string[] = [
        ` ${styleBold(th, th.fg("accent", item.title))}`,
        th.fg("border", "─".repeat(rightWidth)),
      ];
      for (const raw of item.bodyLines) {
        if (!raw) {
          out.push("");
          continue;
        }
        for (const wrapped of wrapTextToWidth(raw, textW)) {
          out.push(` ${wrapped}`);
        }
      }
      this.previewCacheLines = out;
    }

    const maxScroll = Math.max(0, this.previewCacheLines.length - height);
    if (this.previewScroll > maxScroll) this.previewScroll = maxScroll;
    const visible = this.previewCacheLines.slice(this.previewScroll, this.previewScroll + height);
    while (visible.length < height) visible.push("");
    return visible.map((l) => fitWidth(l, rightWidth));
  }

  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    const dragRes = this.dividerDrag.handleMouse(event, {
      totalWidth: this.lastTotalWidth,
      dividerX: this.lastDividerX,
      bodyStartY: this.lastBodyStartY,
      bodyHeight: this.lastBodyHeight,
      minLeft: 24,
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
        if (this.list.move(delta, this.sections.length, this.lastBodyHeight)) {
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
      if (
        y >= this.lastBodyStartY &&
        y < this.lastBodyStartY + this.lastBodyHeight &&
        x >= 1 &&
        x < this.lastDividerX
      ) {
        const idx = this.list.indexAtBodyRow(y - this.lastBodyStartY, this.sections.length);
        if (idx >= 0 && idx < this.sections.length) {
          this.list.select(idx, this.sections.length, this.lastBodyHeight);
          this.previewScroll = 0;
          return { handled: true, render: true };
        }
      }
      return { handled: true, render: false };
    }
    return undefined;
  }

  handleInput(data: string): void {
    if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c") || data === "g" || data === "q") {
      this.finish();
      return;
    }
    if (matchesKey(data, "up") || data === "k") {
      if (this.list.move(-1, this.sections.length, this.lastBodyHeight)) {
        this.previewScroll = 0;
        this.tui.requestRender();
      }
      return;
    }
    if (matchesKey(data, "down") || data === "j") {
      if (this.list.move(1, this.sections.length, this.lastBodyHeight)) {
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
    }
  }

  render(width: number): string[] {
    const termRows = this.getRows();
    const targetRows = Math.max(16, Math.floor(termRows * 0.88));
    const geom = computeSplitGeometry({
      width,
      rows: termRows,
      leftRatio: this.dividerDrag.leftRatio,
      hasSearchBar: false,
      minWidth: 60,
      minLeftWidth: 24,
      minRightWidth: 28,
      heightRatio: 0.88,
      maxRows: targetRows,
    });
    this.lastTotalWidth = geom.totalWidth;
    this.lastBodyHeight = geom.bodyHeight;
    this.lastBodyStartY = geom.bodyStartY;
    this.lastDividerX = geom.dividerX;

    const pills: HeaderPillSpec[] = [
      {
        id: "status",
        label: `[${this.goal.status}]`,
        active: true,
        color: statusColor(this.goal.status),
      },
      {
        id: "tokens",
        label: `[${formatCompactTokens(this.goal.tokensUsed)} tok]`,
        active: false,
      },
    ];

    const frame = renderSplitOverlayFrame({
      geometry: geom,
      titlePrefix: ` ${styleBold(this.theme, this.theme.fg("accent", "Goal Detail"))} `,
      pills,
      leftLines: this.renderLeftPane(geom.leftWidth, geom.bodyHeight),
      rightLines: this.renderRightPane(geom.rightWidth, geom.bodyHeight),
      footerHints: [
        { key: "↑↓/Wheel", label: "select" },
        { key: "PgUp/PgDn", label: "scroll" },
        { key: "Drag │", label: "resize" },
        { key: "Esc/g", label: "close" },
      ],
      theme: this.theme,
    });
    this.lastCloseRect = frame.closeRect;
    return frame.lines.map((l) => (visibleWidth(l) === geom.totalWidth ? l : fitWidth(l, geom.totalWidth)));
  }
}

export function createGoalDetailTui(
  tui: { requestRender: () => void },
  theme: TuiTheme,
  options: GoalDetailTuiOptions,
  done: () => void,
): GoalDetailTui {
  return new GoalDetailTui(tui, theme, options, done);
}
