/**
 * Unified TS TUI design language & layout primitives for grok-pi extensions.
 *
 * Centralizes rounded box borders (`╭─╮│╰─╯` + `┬│┴`), clickable header pill
 * bars with automatic `ClickRegion` hit-testing, Telescope-style search bars,
 * CJK/ANSI display-width padding (`fitWidth`, `wrapTextToWidth`), and footer
 * shortcut hint rendering so every plugin overlay looks and behaves as one family.
 */

import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";

export interface TuiTheme {
  fg: (color: string, text: string) => string;
  bold?: (text: string) => string;
}

export interface ClickRegion {
  x1: number;
  x2: number;
  y: number;
}

/** Coalesce rapid wheel/trackpad events into a single render request. */
export class RenderCoalescer {
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly requestRender: () => void,
    private readonly delayMs = 40,
  ) {}

  get pending(): boolean {
    return this.timer !== null;
  }

  schedule(): void {
    if (this.timer !== null) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      this.requestRender();
    }, this.delayMs);
  }

  cancel(): void {
    if (this.timer === null) return;
    clearTimeout(this.timer);
    this.timer = null;
  }
}

export interface HeaderPillSpec {
  id: string;
  label: string;
  active?: boolean;
  color?: string;
}

export interface FooterHintSpec {
  key: string;
  label: string;
}

export interface SplitFrameGeometry {
  totalWidth: number;
  innerWidth: number;
  leftWidth: number;
  rightWidth: number;
  dividerX: number;
  bodyStartY: number;
  bodyHeight: number;
}

export interface SplitOverlayRenderResult {
  lines: string[];
  geometry: SplitFrameGeometry;
  pillRects: Map<string, ClickRegion>;
  closeRect: ClickRegion;
}

/** Check whether a mouse coordinate `(x, y)` falls inside `region`. */
export function hitRegion(region: ClickRegion | undefined, x: number, y: number): boolean {
  if (!region || region.x1 < 0) return false;
  return y === region.y && x >= region.x1 && x <= region.x2;
}

export function styleBold(theme: TuiTheme, text: string): string {
  return theme.bold ? theme.bold(text) : `\x1b[1m${text}\x1b[0m`;
}

/** Pad or truncate an ANSI-styled string to an exact display column width. */
export function fitWidth(text: string, width: number): string {
  if (width <= 0) return "";
  const truncated = truncateToWidth(text, width, "…");
  const w = visibleWidth(truncated);
  if (w >= width) return truncated;
  return truncated + " ".repeat(width - w);
}

/** Word/character-wrap plain text to `maxWidth` terminal display columns. */
export function wrapTextToWidth(text: string, maxWidth: number): string[] {
  if (maxWidth <= 0) return [""];
  const out: string[] = [];
  for (const rawLine of text.split(/\r?\n/)) {
    if (rawLine.length === 0) {
      out.push("");
      continue;
    }
    let current = "";
    let currentW = 0;
    for (const ch of rawLine) {
      const cw = visibleWidth(ch);
      if (currentW + cw > maxWidth && current.length > 0) {
        out.push(current);
        current = ch;
        currentW = cw;
      } else {
        current += ch;
        currentW += cw;
      }
    }
    if (current.length > 0) out.push(current);
  }
  return out.length > 0 ? out : [""];
}

/** Highlight multi-term search matches inside a line while preserving ANSI escapes. */
export function highlightTerms(
  text: string,
  query: string,
  highlight: (match: string) => string = (m) => `\x1b[7m${m}\x1b[27m`,
): string {
  const terms = query
    .trim()
    .split(/\s+/)
    .filter((t) => t.length >= 2);
  if (terms.length === 0) return text;
  const escaped = terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  const re = new RegExp(`(${escaped.join("|")})`, "gi");
  if (!text.includes("\x1b")) {
    return text.replace(re, (match) => highlight(match));
  }
  const ansiRe = /(\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\))/g;
  return text
    .split(ansiRe)
    .map((segment, index) => (index % 2 === 1 ? segment : segment.replace(re, (match) => highlight(match))))
    .join("");
}

/** Compute consistent responsive geometry for a two-pane bordered overlay. */
export function computeSplitGeometry(options: {
  width: number;
  rows: number;
  leftRatio?: number;
  hasSearchBar: boolean;
  minWidth?: number;
  minLeftWidth?: number;
  minRightWidth?: number;
  heightRatio?: number;
  maxRows?: number;
}): SplitFrameGeometry {
  const minWidth = options.minWidth ?? 56;
  const minLeft = options.minLeftWidth ?? 24;
  const minRight = options.minRightWidth ?? 24;
  const totalWidth = Math.max(minWidth, Math.floor(options.width));
  const innerWidth = totalWidth - 2;
  const leftRatio = options.leftRatio ?? 0.4;
  const leftWidth = Math.max(
    minLeft,
    Math.min(innerWidth - minRight - 1, Math.floor(innerWidth * leftRatio)),
  );
  const rightWidth = Math.max(minRight, innerWidth - leftWidth - 1);
  const dividerX = 1 + leftWidth;

  const heightRatio = options.heightRatio ?? 0.84;
  const maxRows = options.maxRows ?? 34;
  const totalRows = Math.max(16, Math.min(maxRows, Math.floor(options.rows * heightRatio)));
  // With search bar: top(0)+header(1)+midRule(2)+search(3)+splitTop(4)+splitBottom+footer+bottom = 8
  // Without search bar: top(0)+header(1)+splitTop(2)+splitBottom+footer+bottom = 6
  const chromeRows = options.hasSearchBar ? 8 : 6;
  const bodyStartY = options.hasSearchBar ? 5 : 3;
  const bodyHeight = Math.max(8, totalRows - chromeRows);

  return {
    totalWidth,
    innerWidth,
    leftWidth,
    rightWidth,
    dividerX,
    bodyStartY,
    bodyHeight,
  };
}

/** Render a unified header row with title, clickable pills, and right-aligned `[✕]`. */
export function renderHeaderRow(options: {
  innerWidth: number;
  titlePrefix: string;
  pills: readonly HeaderPillSpec[];
  theme: TuiTheme;
  y?: number;
}): { line: string; pillRects: Map<string, ClickRegion>; closeRect: ClickRegion } {
  const { innerWidth, titlePrefix, pills, theme, y = 1 } = options;
  const b = (s: string) => theme.fg("border", s);
  const pillRects = new Map<string, ClickRegion>();

  let col = 1 + visibleWidth(titlePrefix);
  const renderedPills: string[] = [];
  for (const pill of pills) {
    if (!pill.label) continue;
    const w = visibleWidth(pill.label);
    const start = col;
    const end = start + w - 1;
    pillRects.set(pill.id, { x1: start, x2: end, y });
    col = end + 2;
    const color = pill.color ?? (pill.active ? "accent" : "dim");
    const styled = pill.active
      ? styleBold(theme, theme.fg(color, pill.label))
      : theme.fg(color, pill.label);
    renderedPills.push(styled);
  }

  const leftHeader = `${titlePrefix}${renderedPills.join(" ")}`;
  const closeText = "[✕]";
  const closeW = visibleWidth(closeText);
  const gap = Math.max(1, innerWidth - visibleWidth(leftHeader) - closeW - 1);
  const closeStart = 1 + innerWidth - closeW - 1;
  const closeRect: ClickRegion = { x1: closeStart, x2: closeStart + closeW - 1, y };

  const line = `${b("│")}${fitWidth(`${leftHeader}${" ".repeat(gap)}${theme.fg("dim", closeText)} `, innerWidth)}${b("│")}`;
  return { line, pillRects, closeRect };
}

/** Render a unified search input row with reverse cursor and right-aligned counter. */
export function renderSearchRow(options: {
  innerWidth: number;
  promptPrefix: string;
  query: string;
  placeholder: string;
  rightLabel: string;
  theme: TuiTheme;
}): string {
  const { innerWidth, promptPrefix, query, placeholder, rightLabel, theme } = options;
  const b = (s: string) => theme.fg("border", s);
  const queryDisplay = query
    ? `${query}\x1b[7m \x1b[27m`
    : `\x1b[7m \x1b[27m${theme.fg("dim", placeholder)}`;
  const budget = Math.max(10, innerWidth - visibleWidth(rightLabel) - 1);
  const left = fitWidth(`${promptPrefix}${queryDisplay}`, budget);
  return `${b("│")}${fitWidth(`${left} ${theme.fg("dim", rightLabel)}`, innerWidth)}${b("│")}`;
}

/** Render a unified footer row from structured key/label hint pairs. */
export function renderFooterRow(options: {
  innerWidth: number;
  hints?: readonly FooterHintSpec[];
  customContent?: string;
  theme: TuiTheme;
}): string {
  const { innerWidth, hints = [], customContent, theme } = options;
  const b = (s: string) => theme.fg("border", s);
  const content =
    customContent ??
    ` ${hints.map((h) => `${theme.fg("accent", h.key)}${theme.fg("dim", `:${h.label}`)}`).join("  ")}`;
  return `${b("│")}${fitWidth(content, innerWidth)}${b("│")}`;
}

/** Assemble a complete bordered two-pane overlay frame from left/right body renderers. */
export function renderSplitOverlayFrame(options: {
  geometry: SplitFrameGeometry;
  titlePrefix: string;
  pills: readonly HeaderPillSpec[];
  search?: {
    promptPrefix: string;
    query: string;
    placeholder: string;
    rightLabel: string;
  };
  leftLines: readonly string[];
  rightLines: readonly string[];
  footerHints?: readonly FooterHintSpec[];
  footerCustom?: string;
  theme: TuiTheme;
}): SplitOverlayRenderResult {
  const { geometry: g, theme } = options;
  const b = (s: string) => theme.fg("border", s);
  const topBorder = b(`╭${"─".repeat(g.innerWidth)}╮`);
  const midRule = b(`├${"─".repeat(g.innerWidth)}┤`);
  const splitTop = b(`├${"─".repeat(g.leftWidth)}┬${"─".repeat(g.rightWidth)}┤`);
  const splitBottom = b(`├${"─".repeat(g.leftWidth)}┴${"─".repeat(g.rightWidth)}┤`);
  const bottomBorder = b(`╰${"─".repeat(g.innerWidth)}╯`);

  const { line: headerLine, pillRects, closeRect } = renderHeaderRow({
    innerWidth: g.innerWidth,
    titlePrefix: options.titlePrefix,
    pills: options.pills,
    theme,
    y: 1,
  });

  const bodyLines: string[] = [];
  for (let i = 0; i < g.bodyHeight; i++) {
    const left = options.leftLines[i] ?? " ".repeat(g.leftWidth);
    const right = options.rightLines[i] ?? " ".repeat(g.rightWidth);
    bodyLines.push(`${b("│")}${left}${b("│")}${right}${b("│")}`);
  }

  const footerLine = renderFooterRow({
    innerWidth: g.innerWidth,
    hints: options.footerHints,
    customContent: options.footerCustom,
    theme,
  });

  const lines = options.search
    ? [
        topBorder,
        headerLine,
        midRule,
        renderSearchRow({
          innerWidth: g.innerWidth,
          promptPrefix: options.search.promptPrefix,
          query: options.search.query,
          placeholder: options.search.placeholder,
          rightLabel: options.search.rightLabel,
          theme,
        }),
        splitTop,
        ...bodyLines,
        splitBottom,
        footerLine,
        bottomBorder,
      ]
    : [topBorder, headerLine, splitTop, ...bodyLines, splitBottom, footerLine, bottomBorder];

  return { lines, geometry: g, pillRects, closeRect };
}
