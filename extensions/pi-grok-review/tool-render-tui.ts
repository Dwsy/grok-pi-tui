/**
 * Custom Pi plugin tool renderer popup (`__pi_tool_render`).
 *
 * Activated only when F2 `pi_custom_tool_renderer` is enabled (default OFF) and
 * the user presses Enter on a non-builtin custom tool call (`Other` / `UseTool`).
 * Built-in adapted tools (`read`, `edit`, `write`, `bash`, etc.) always keep
 * their native Rust viewers.
 *
 * Looks up the plugin's registered `renderCall` / `renderResult` via the live
 * `ExtensionRunner` captured by `pi-grok-rpc-compat` and renders the Pi TUI
 * components inside a 90vh bordered overlay with instant `Pi Element ↔ JSON`
 * switching (`Tab` / `Ctrl+J` / click pill).
 */

import fs from "node:fs";
import {
  matchesKey,
  visibleWidth,
  type Component,
  type TuiMouseEvent,
  type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import {
  fitWidth,
  hitRegion,
  RenderCoalescer,
  renderFooterRow,
  renderHeaderRow,
  styleBold,
  wrapTextToWidth,
  type ClickRegion,
  type TuiTheme,
} from "../pi-grok-tui-kit/index.ts";

export interface PiToolRenderersLike {
  renderCall?: (args: unknown, theme: unknown, context: Record<string, unknown>) => Component | undefined;
  renderResult?: (
    result: { content: unknown[]; details?: unknown },
    options: { expanded: boolean; isPartial: boolean },
    theme: unknown,
    context: Record<string, unknown>,
  ) => Component | undefined;
}

export type PiMessageRendererLike = (
  message: Record<string, unknown>,
  options: { expanded: boolean },
  theme: unknown,
) => Component | undefined;

export interface PiExtensionRunnerLike {
  getToolDefinition?: (toolName: string) => PiToolRenderersLike | undefined;
  resolveToolRenderers?: (
    toolName: string,
    base: () => PiToolRenderersLike | undefined,
  ) => PiToolRenderersLike | undefined;
  getMessageRenderer?: (customType: string) => PiMessageRendererLike | undefined;
}

export interface ParsedToolInvocation {
  toolCallId: string;
  toolName: string;
  args: unknown;
  result?: { content: unknown[]; details?: unknown };
  isError: boolean;
  timestamp: string;
}

export interface ToolRenderTuiOptions {
  invocation: ParsedToolInvocation;
  cwd: string;
  resolveRenderers?: (toolName: string) => PiToolRenderersLike | undefined;
  getRows?: () => number;
}

/** Resolve Pi's custom tool renderers from the live captured `ExtensionRunner`. */
export function resolveLivePiToolRenderers(toolName: string): PiToolRenderersLike | undefined {
  const runner = (
    globalThis as typeof globalThis & { __piGrokExtensionRunner?: PiExtensionRunnerLike }
  ).__piGrokExtensionRunner;
  if (!runner || !toolName) return undefined;
  try {
    if (typeof runner.resolveToolRenderers === "function") {
      return runner.resolveToolRenderers(toolName, () => runner.getToolDefinition?.(toolName));
    }
    return runner.getToolDefinition?.(toolName);
  } catch {
    return undefined;
  }
}

/** Resolve Pi's custom message renderer (`registerMessageRenderer`) from the live runner. */
export function resolveLivePiMessageRenderer(customType: string): PiMessageRendererLike | undefined {
  const runner = (
    globalThis as typeof globalThis & { __piGrokExtensionRunner?: PiExtensionRunnerLike }
  ).__piGrokExtensionRunner;
  if (!runner || !customType) return undefined;
  try {
    return runner.getMessageRenderer?.(customType);
  } catch {
    return undefined;
  }
}

/**
 * Render a Pi custom tool invocation (`renderCall` + `renderResult`) into
 * terminal lines with safe error isolation. Returns `null` when no custom
 * renderer is registered or rendering yields no output.
 */
export function renderPiToolElement(options: {
  invocation: ParsedToolInvocation;
  width: number;
  theme: TuiTheme;
  cwd: string;
  resolveRenderers?: (toolName: string) => PiToolRenderersLike | undefined;
  onInvalidate?: () => void;
}): string[] | null {
  const resolve = options.resolveRenderers ?? resolveLivePiToolRenderers;
  const renderers = resolve(options.invocation.toolName);
  if (!renderers || (!renderers.renderCall && !renderers.renderResult)) return null;

  const innerW = Math.max(12, options.width - 2);
  const renderContext: Record<string, unknown> = {
    args: options.invocation.args,
    toolCallId: options.invocation.toolCallId,
    invalidate: () => options.onInvalidate?.(),
    lastComponent: undefined,
    state: {},
    cwd: options.cwd,
    executionStarted: true,
    argsComplete: true,
    isPartial: false,
    expanded: true,
    showImages: false,
    isError: options.invocation.isError,
  };

  const out: string[] = [];
  if (typeof renderers.renderCall === "function") {
    try {
      const comp = renderers.renderCall(options.invocation.args, options.theme, renderContext);
      if (comp && typeof comp.render === "function") {
        for (const line of comp.render(innerW)) out.push(` ${line}`);
      }
    } catch {
      // Ignore throwing plugin renderCall and try renderResult / fallback.
    }
  }

  if (options.invocation.result && typeof renderers.renderResult === "function") {
    try {
      const comp = renderers.renderResult(
        options.invocation.result,
        { expanded: true, isPartial: false },
        options.theme,
        renderContext,
      );
      if (comp && typeof comp.render === "function") {
        if (out.length > 0) out.push("");
        for (const line of comp.render(innerW)) out.push(` ${line}`);
      }
    } catch {
      // Ignore throwing plugin renderResult.
    }
  }

  return out.length > 0 ? out : null;
}

/**
 * Render a Pi custom message (`registerMessageRenderer`) into terminal lines.
 * Returns `null` when no renderer is registered or rendering fails.
 */
export function renderPiCustomMessageElement(options: {
  customType: string;
  message: Record<string, unknown>;
  width: number;
  theme: TuiTheme;
  resolveMessageRenderer?: (customType: string) => PiMessageRendererLike | undefined;
}): string[] | null {
  const resolve = options.resolveMessageRenderer ?? resolveLivePiMessageRenderer;
  const renderer = resolve(options.customType);
  if (typeof renderer !== "function") return null;
  const innerW = Math.max(12, options.width - 2);
  try {
    const comp = renderer(options.message, { expanded: true }, options.theme);
    if (!comp || typeof comp.render !== "function") return null;
    const lines = comp.render(innerW);
    return lines.length > 0 ? lines.map((l) => ` ${l}`) : null;
  } catch {
    return null;
  }
}

/** In-memory per-tool view preference (`Pi Element` vs `JSON`) for the current session. */
const preferredToolModes = new Map<string, "pi" | "json">();

export function clearPreferredToolModes(): void {
  preferredToolModes.clear();
}
/** Locate a single tool call + its paired tool result in the session JSONL by `toolCallId`. */
export function findToolInvocationInJsonl(
  jsonlPath: string,
  toolCallId: string,
): ParsedToolInvocation | null {
  if (!jsonlPath || !toolCallId || !fs.existsSync(jsonlPath)) return null;
  let raw: string;
  try {
    raw = fs.readFileSync(jsonlPath, "utf8");
  } catch {
    return null;
  }

  let found: ParsedToolInvocation | null = null;
  for (const line of raw.split("\n")) {
    const trimmed = line.trim();
    // Fast substring pre-filter skips >99% of unrelated JSONL lines before JSON.parse.
    if (!trimmed || trimmed[0] !== "{" || !trimmed.includes(toolCallId)) continue;
    let obj: Record<string, unknown>;
    try {
      obj = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      continue;
    }
    if (obj.type !== "message") continue;
    const msg = (obj.message ?? obj) as {
      role?: unknown;
      content?: unknown;
      toolCallId?: unknown;
      tool_call_id?: unknown;
      toolName?: unknown;
      details?: unknown;
      isError?: unknown;
    };
    const role = typeof msg.role === "string" ? msg.role.toLowerCase() : "";
    const ts = typeof obj.timestamp === "string" ? obj.timestamp : "";

    if (role === "assistant" && Array.isArray(msg.content) && !found) {
      for (const block of msg.content) {
        if (!block || typeof block !== "object") continue;
        const item = block as {
          type?: unknown;
          id?: unknown;
          name?: unknown;
          arguments?: unknown;
          input?: unknown;
        };
        const bType = typeof item.type === "string" ? item.type.toLowerCase() : "";
        if (bType !== "toolcall" && bType !== "tool_call" && bType !== "tool_use") continue;
        if (item.id === toolCallId) {
          found = {
            toolCallId,
            toolName: typeof item.name === "string" && item.name ? item.name : "tool",
            args: item.arguments ?? item.input ?? {},
            isError: false,
            timestamp: ts,
          };
          break;
        }
      }
    } else if (role === "toolresult" || role === "tool_result") {
      const id = String(msg.toolCallId ?? msg.tool_call_id ?? "").trim();
      if (id !== toolCallId) continue;
      const content = Array.isArray(msg.content)
        ? msg.content
        : typeof msg.content === "string"
          ? [{ type: "text", text: msg.content }]
          : [];
      const resultPayload = { content, details: msg.details };
      if (found) {
        found.result = resultPayload;
        found.isError = msg.isError === true;
      } else {
        found = {
          toolCallId,
          toolName: typeof msg.toolName === "string" && msg.toolName ? msg.toolName : "tool",
          args: {},
          result: resultPayload,
          isError: msg.isError === true,
          timestamp: ts,
        };
      }
    }
  }
  return found;
}

function prettyJson(value: unknown): string {
  if (value === undefined) return "(none)";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

export class ToolRenderTui implements Component {
  private readonly tui: { requestRender: () => void };
  private readonly theme: TuiTheme;
  private readonly invocation: ParsedToolInvocation;
  private readonly cwd: string;
  private readonly resolveRenderers: (toolName: string) => PiToolRenderersLike | undefined;
  private readonly getRows: () => number;
  private readonly done: () => void;

  private mode: "pi" | "json" = "pi";
  private scroll = 0;
  private lastTotalLines = 0;
  private bodyStartY = 3;
  private bodyHeight = 14;
  private pillRects = new Map<string, ClickRegion>();
  private closeRect: ClickRegion = { x1: -1, x2: -1, y: 1 };
  private cacheKey = "";
  private cachedLines: string[] = [];
  private usedFallbackJson = false;
  private readonly renderCoalescer: RenderCoalescer;

  constructor(
    tui: { requestRender: () => void },
    theme: TuiTheme,
    options: ToolRenderTuiOptions,
    done: () => void,
  ) {
    this.tui = tui;
    this.theme = theme;
    this.invocation = options.invocation;
    this.cwd = options.cwd;
    this.resolveRenderers = options.resolveRenderers ?? resolveLivePiToolRenderers;
    this.getRows = options.getRows ?? (() => 28);
    this.done = done;
    this.mode = preferredToolModes.get(options.invocation.toolName) ?? "pi";
    this.renderCoalescer = new RenderCoalescer(() => this.tui.requestRender());
  }

  getMode(): "pi" | "json" {
    return this.mode;
  }

  usedJsonFallback(): boolean {
    return this.usedFallbackJson;
  }

  invalidate(): void {
    this.cacheKey = "";
  }

  private finish(): void {
    this.renderCoalescer.cancel();
    this.done();
  }

  private scrollBy(delta: number): void {
    const maxScroll = Math.max(0, this.lastTotalLines - this.bodyHeight);
    this.scroll = Math.max(0, Math.min(maxScroll, this.scroll + delta));
  }

  private buildJsonLines(width: number): string[] {
    const th = this.theme;
    const colorJson = (line: string) =>
      line.replace(
        /("(?:\\u[0-9a-fA-F]{4}|\\[^u]|[^\\"])*"(\s*:)?|\b(?:true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|[{}\[\],:])/g,
        (match, _tok, isKey) => {
          if (isKey) {
            const c = match.lastIndexOf(":");
            return `${th.fg("accent", match.slice(0, c))}${th.fg("dim", match.slice(c))}`;
          }
          if (match.startsWith('"')) return th.fg("success", match);
          if (match === "true" || match === "false" || match === "null") return th.fg("warning", match);
          if (/^[{}\[\],:]$/.test(match)) return th.fg("dim", match);
          return th.fg("warning", match);
        },
      );
    const out: string[] = [
      ` ${styleBold(th, th.fg("accent", "Input"))}`,
      ...wrapTextToWidth(prettyJson(this.invocation.args), Math.max(10, width - 2)).map(
        (l) => `  ${colorJson(l)}`,
      ),
      "",
      ` ${styleBold(th, th.fg(this.invocation.isError ? "error" : "success", this.invocation.isError ? "Output (error)" : "Output"))}`,
      ...wrapTextToWidth(
        this.invocation.result ? prettyJson(this.invocation.result) : "(pending)",
        Math.max(10, width - 2),
      ).map((l) => `  ${colorJson(l)}`),
    ];
    return out;
  }

  private buildPiElementLines(width: number): { lines: string[]; fallback: boolean } {
    const lines = renderPiToolElement({
      invocation: this.invocation,
      width,
      theme: this.theme,
      cwd: this.cwd,
      resolveRenderers: this.resolveRenderers,
      onInvalidate: () => {
        this.cacheKey = "";
        this.tui.requestRender();
      },
    });
    if (!lines) {
      return { lines: this.buildJsonLines(width), fallback: true };
    }
    return { lines, fallback: false };
  }

  handleInput(data: string): void {
    if (matchesKey(data, "escape") || matchesKey(data, "ctrl+c")) {
      this.finish();
      return;
    }
    if (matchesKey(data, "tab") || matchesKey(data, "ctrl+j")) {
      this.mode = this.mode === "pi" ? "json" : "pi";
      preferredToolModes.set(this.invocation.toolName, this.mode);
      this.scroll = 0;
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "up") || data === "k") {
      this.scrollBy(-1);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "down") || data === "j") {
      this.scrollBy(1);
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "pageUp") || matchesKey(data, "ctrl+u")) {
      this.scrollBy(-Math.max(4, Math.floor(this.bodyHeight / 2)));
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "pageDown") || matchesKey(data, "ctrl+d")) {
      this.scrollBy(Math.max(4, Math.floor(this.bodyHeight / 2)));
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "home")) {
      this.scroll = 0;
      this.tui.requestRender();
      return;
    }
    if (matchesKey(data, "end")) {
      this.scroll = Math.max(0, this.lastTotalLines - this.bodyHeight);
      this.tui.requestRender();
    }
  }

  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    const { x, y, type } = event;
    if (type === "wheel") {
      const delta = (event.wheelDelta ?? 0) > 0 ? 1 : -1;
      if (y >= this.bodyStartY && y < this.bodyStartY + this.bodyHeight) {
        this.scrollBy(delta * 3);
        this.renderCoalescer.schedule();
      }
      return { handled: true, render: false };
    }
    if (type === "press" && event.button === "left") {
      if (hitRegion(this.closeRect, x, y)) {
        this.finish();
        return { handled: true, render: false };
      }
      if (hitRegion(this.pillRects.get("pi"), x, y)) {
        this.mode = "pi";
        preferredToolModes.set(this.invocation.toolName, "pi");
        this.scroll = 0;
        return { handled: true, render: true };
      }
      if (hitRegion(this.pillRects.get("json"), x, y)) {
        this.mode = "json";
        preferredToolModes.set(this.invocation.toolName, "json");
        this.scroll = 0;
        return { handled: true, render: true };
      }
    }
    return { handled: true, render: false };
  }

  render(width: number): string[] {
    const totalWidth = Math.max(56, Math.floor(width));
    const innerWidth = totalWidth - 2;
    const termRows = this.getRows();
    const totalRows = Math.max(16, Math.floor(termRows * 0.9));
    // top(0) + header(1) + mid(2) + body + mid(n-3) + footer(n-2) + bottom(n-1) = 6 chrome rows
    this.bodyStartY = 3;
    this.bodyHeight = Math.max(8, totalRows - 6);

    const key = `${this.mode}:${innerWidth}`;
    if (key !== this.cacheKey) {
      this.cacheKey = key;
      if (this.mode === "pi") {
        const res = this.buildPiElementLines(innerWidth);
        this.cachedLines = res.lines;
        this.usedFallbackJson = res.fallback;
      } else {
        this.cachedLines = this.buildJsonLines(innerWidth);
        this.usedFallbackJson = false;
      }
    }

    this.lastTotalLines = this.cachedLines.length;
    const maxScroll = Math.max(0, this.lastTotalLines - this.bodyHeight);
    if (this.scroll > maxScroll) this.scroll = maxScroll;

    const b = (s: string) => this.theme.fg("border", s);
    const header = renderHeaderRow({
      innerWidth,
      titlePrefix: ` ${styleBold(this.theme, this.theme.fg("accent", this.invocation.toolName))} ${this.theme.fg("dim", `(${this.invocation.toolCallId.slice(0, 12)})`)}  `,
      pills: [
        {
          id: "pi",
          label: this.usedFallbackJson ? "[Pi Element · fallback]" : "[Pi Element]",
          active: this.mode === "pi",
        },
        { id: "json", label: "[JSON]", active: this.mode === "json" },
      ],
      theme: this.theme,
      y: 1,
    });
    this.pillRects = header.pillRects;
    this.closeRect = header.closeRect;

    const visible = this.cachedLines.slice(this.scroll, this.scroll + this.bodyHeight);
    while (visible.length < this.bodyHeight) visible.push("");
    const bodyLines = visible.map((l) => `${b("│")}${fitWidth(l, innerWidth)}${b("│")}`);

    const scrollHint =
      this.lastTotalLines > this.bodyHeight
        ? `${this.scroll + 1}-${Math.min(this.lastTotalLines, this.scroll + this.bodyHeight)}/${this.lastTotalLines}`
        : `${this.lastTotalLines} lines`;
    const footer = renderFooterRow({
      innerWidth,
      hints: [
        { key: "Tab/Ctrl+J", label: "Pi Element/JSON" },
        { key: "↑↓/PgUp/PgDn/Wheel", label: `scroll (${scrollHint})` },
        { key: "Esc", label: "close" },
      ],
      theme: this.theme,
    });

    const lines = [
      b(`╭${"─".repeat(innerWidth)}╮`),
      header.line,
      b(`├${"─".repeat(innerWidth)}┤`),
      ...bodyLines,
      b(`├${"─".repeat(innerWidth)}┤`),
      footer,
      b(`╰${"─".repeat(innerWidth)}╯`),
    ];
    return lines.map((line) => (visibleWidth(line) === totalWidth ? line : fitWidth(line, totalWidth)));
  }
}

export function createToolRenderTui(
  tui: { requestRender: () => void },
  theme: TuiTheme,
  options: ToolRenderTuiOptions,
  done: () => void,
): ToolRenderTui {
  return new ToolRenderTui(tui, theme, options, done);
}
