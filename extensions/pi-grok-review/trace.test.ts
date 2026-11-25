import { describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { TuiTheme } from "../pi-grok-tui-kit/index.ts";
import {
  countTraceKinds,
  filterTraceEntries,
  parseSessionTraceJsonl,
} from "./trace-data.ts";
import { createTraceTui } from "./trace-tui.ts";
import {
  clearPreferredToolModes,
  createToolRenderTui,
  findToolInvocationInJsonl,
  renderPiCustomMessageElement,
} from "./tool-render-tui.ts";

const plainTheme: TuiTheme = {
  fg: (_color: string, text: string) => text,
  bold: (text: string) => text,
};

function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;]*m/g, "");
}

function writeSampleTraceJsonl(dir: string): string {
  const file = path.join(dir, "trace-session.jsonl");
  const lines = [
    JSON.stringify({
      type: "session",
      id: "sess-trace-1",
      cwd: "/Users/dev/pi-grok-build",
      timestamp: "2026-11-25T04:00:00.000Z",
    }),
    JSON.stringify({
      type: "model_change",
      id: "mc-1",
      provider: "anthropic",
      modelId: "claude-sonnet-4-5",
      timestamp: "2026-11-25T04:00:01.000Z",
    }),
    JSON.stringify({
      type: "message",
      id: "m-user-1",
      timestamp: "2026-11-25T04:01:00.000Z",
      message: {
        role: "user",
        content: [{ type: "text", text: "Check git status and inspect README" }],
      },
    }),
    JSON.stringify({
      type: "message",
      id: "m-asst-1",
      timestamp: "2026-11-25T04:01:05.000Z",
      message: {
        role: "assistant",
        provider: "anthropic",
        model: "claude-sonnet-4-5",
        usage: { input: 1000, output: 200, cacheRead: 300, cacheWrite: 0 },
        content: [
          { type: "thinking", thinking: "Let me run git status first." },
          { type: "toolCall", id: "call_bash_1", name: "bash", arguments: { command: "git status" } },
          { type: "text", text: "Checking repository status now." },
        ],
      },
    }),
    JSON.stringify({
      type: "message",
      id: "m-tr-1",
      timestamp: "2026-11-25T04:01:06.000Z",
      message: {
        role: "toolResult",
        toolCallId: "call_bash_1",
        toolName: "bash",
        isError: false,
        content: [{ type: "text", text: "On branch main\nnothing to commit" }],
      },
    }),
    JSON.stringify({
      type: "session_info",
      id: "si-1",
      parentId: null,
      timestamp: "2026-11-25T04:02:00.000Z",
      name: "Trace inspection session",
    }),
  ];
  fs.writeFileSync(file, lines.join("\n") + "\n", "utf8");
  return file;
}

describe("trace-data JSONL parsing and filtering", () => {
  test("parses full JSONL entries, pairs toolCall with toolResult, and counts kinds", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-trace-data-"));
    try {
      const file = writeSampleTraceJsonl(dir);
      const entries = parseSessionTraceJsonl(file);
      // session + model_change + user + thinking + toolCall(paired with toolResult) + assistant + session_info = 7
      expect(entries).toHaveLength(7);
      const counts = countTraceKinds(entries);
      expect(counts).toEqual({
        all: 7,
        tool: 1,
        user: 1,
        assistant: 1,
        thinking: 1,
        meta: 3,
      });

      const tool = entries.find((e) => e.toolCallId === "call_bash_1")!;
      expect(tool).toBeDefined();
      expect(tool.label).toContain("bash · git status");
      expect(tool.toolInput).toContain("git status");
      expect(tool.toolOutput).toContain("On branch main");
      expect(tool.previewBody).toContain("── Input ──");
      expect(tool.previewBody).toContain("── Output (ok) ──");

      expect(filterTraceEntries(entries, { filter: "tool", query: "nothing to commit" })).toHaveLength(1);
      expect(filterTraceEntries(entries, { filter: "meta", query: "session_info" })).toHaveLength(1);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("TraceTui split overlay rendering, filter, search, raw toggle, and divider drag", () => {
  test("pre-selects initialToolCallId, supports Tab filter, live search, Ctrl+J Raw JSON, and divider drag", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-trace-tui-"));
    try {
      const entries = parseSessionTraceJsonl(writeSampleTraceJsonl(dir));
      let closed = false;
      let renderRequests = 0;
      const tui = createTraceTui(
        { requestRender: () => { renderRequests += 1; } },
        plainTheme,
        {
          entries,
          initialToolCallId: "call_bash_1",
          getRows: () => 28,
        },
        () => {
          closed = true;
        },
      );

      expect(tui.getSelectedEntry()?.toolCallId).toBe("call_bash_1");
      const width = 112;
      const lines = tui.render(width);
      for (const line of lines) {
        expect(visibleWidth(line)).toBe(width);
      }
      const plain = lines.map(stripAnsi).join("\n");
      expect(plain).toContain("/trace");
      expect(plain).toContain("bash · git status");
      expect(plain).toContain("── Input ──");
      expect(plain).toContain("On branch main");

      // Ctrl+J toggles Pretty -> Pi Element -> Raw JSON.
      tui.handleInput("\n"); // Ctrl+J
      expect(tui.getViewMode()).toBe("pi");
      expect(tui.render(width).map(stripAnsi).join("\n")).toContain("[Pi Element]");
      tui.handleInput("\n"); // Ctrl+J
      expect(tui.getViewMode()).toBe("raw");
      expect(tui.render(width).map(stripAnsi).join("\n")).toContain("[Raw JSON]");

      // Tab cycles filter All -> Tools -> User.
      tui.handleInput("\t");
      expect(tui.getFilter()).toBe("tool");
      tui.handleInput("\t");
      expect(tui.getFilter()).toBe("user");
      expect(tui.getSelectedEntry()?.kind).toBe("user");

      // Search filters live.
      tui.handleInput("\x1b[Z"); // shift+tab -> tool
      tui.handleInput("\x1b[Z"); // shift+tab -> all
      expect(tui.getFilter()).toBe("all");
      tui.handleInput("README");
      expect(tui.getQuery()).toBe("README");
      expect(tui.getSelectedEntry()?.kind).toBe("user");

      // Right-pane wheel coalesces renders.
      const beforeRenders = renderRequests;
      const wheelEvent: TuiMouseEvent = {
        type: "wheel",
        button: "none",
        wheelDelta: 1,
        x: 90,
        y: 10,
        screenX: 90,
        screenY: 10,
        width,
        height: lines.length,
        shift: false,
        alt: false,
        ctrl: false,
      };
      tui.handleMouse(wheelEvent);
      tui.handleMouse(wheelEvent);
      expect(renderRequests).toBe(beforeRenders);
      await new Promise((r) => setTimeout(r, 70));
      expect(renderRequests).toBe(beforeRenders + 1);

      // Esc closes cleanly.
      tui.handleInput("\x1b");
      expect(closed).toBe(true);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("syntax-highlights Pretty section/JSON and Raw JSON lines with exact width", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-trace-hl-"));
    try {
      const file = writeSampleTraceJsonl(dir);
      const entries = parseSessionTraceJsonl(file);
      const ansiTheme: TuiTheme = {
        fg: (color, text) => `\x1b[38;5;${color === "accent" ? 39 : color === "success" ? 42 : color === "warning" ? 214 : color === "error" ? 196 : 244}m${text}\x1b[0m`,
        bold: (text) => `\x1b[1m${text}\x1b[22m`,
      };
      let copiedText = "";
      const tui = createTraceTui(
        { requestRender: () => {} },
        ansiTheme,
        {
          entries,
          initialToolCallId: "call_bash_1",
          onCopy: (text) => {
            copiedText = text;
          },
          getRows: () => 26,
        },
        () => {},
      );
      const width = 104;
      const prettyLines = tui.render(width);
      for (const line of prettyLines) expect(visibleWidth(line)).toBe(width);
      const prettyJoined = prettyLines.join("\n");
      // Type badge in left pane and Type value in right pane are bolded.
      expect(prettyJoined).toContain("\x1b[1m\x1b[38;5;214m[tool] \x1b[0m\x1b[22m");
      expect(prettyJoined).toContain("\x1b[1m\x1b[38;5;214mtoolCall\x1b[0m\x1b[22m");
      // Pretty mode highlights section headers and JSON keys/strings inside Input.
      expect(prettyJoined).toContain("\x1b[38;5;39m── Input ──\x1b[0m");
      expect(prettyJoined).toContain('\x1b[38;5;39m"command"\x1b[0m');
      expect(prettyJoined).toContain('\x1b[38;5;42m"git status"\x1b[0m');

      // Raw JSON mode highlights JSON keys/strings cleanly alongside search highlight.
      tui.handleInput("\n"); // pretty -> pi
      tui.handleInput("\n"); // pi -> raw
      tui.handleInput("bash");
      const rawLines = tui.render(width);
      for (const line of rawLines) expect(visibleWidth(line)).toBe(width);
      expect(rawLines.join("\n")).toContain('\x1b[38;5;39m"type"\x1b[0m');
      expect(rawLines.join("\n")).toContain("\x1b[7mbash\x1b[27m");

      // Ctrl+X copies current Raw JSON / preview payload and shows confirmation.
      tui.handleInput("\x18");
      expect(copiedText).toContain("git status");
      expect(tui.render(width).map(stripAnsi).join("\n")).toContain("Copied #");
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("ToolRenderTui custom Pi plugin tool renderer popup", () => {
  test("renders plugin renderCall/renderResult, toggles Pi Element <-> JSON, and falls back gracefully", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-tool-render-"));
    try {
      const file = writeSampleTraceJsonl(dir);
      const inv = findToolInvocationInJsonl(file, "call_bash_1");
      expect(inv).not.toBeNull();
      expect(inv!.toolName).toBe("bash");
      expect(inv!.result?.content).toHaveLength(1);

      let closed = false;
      const tui = createToolRenderTui(
        { requestRender: () => {} },
        plainTheme,
        {
          invocation: { ...inv!, toolName: "custom_plugin_tool" },
          cwd: dir,
          resolveRenderers: (name) =>
            name === "custom_plugin_tool"
              ? {
                  renderCall: (args) => ({
                    invalidate() {},
                    render: () => [`[PI CALL] ${JSON.stringify(args)}`],
                  }),
                  renderResult: () => ({
                    invalidate() {},
                    render: () => ["[PI RESULT] custom widget output"],
                  }),
                }
              : undefined,
          getRows: () => 24,
        },
        () => {
          closed = true;
        },
      );

      const width = 100;
      const piLines = tui.render(width);
      for (const line of piLines) expect(visibleWidth(line)).toBe(width);
      const piPlain = piLines.map(stripAnsi).join("\n");
      expect(piPlain).toContain("[PI CALL]");
      expect(piPlain).toContain("[PI RESULT] custom widget output");
      expect(tui.usedJsonFallback()).toBe(false);

      // Tab switches to JSON view.
      tui.handleInput("\t");
      expect(tui.getMode()).toBe("json");
      const jsonPlain = tui.render(width).map(stripAnsi).join("\n");
      expect(jsonPlain).toContain("On branch main");

      // Esc closes.
      tui.handleInput("\x1b");
      expect(closed).toBe(true);

      // Reopening the same tool remembers the session's JSON preference until cleared.
      const reopened = createToolRenderTui(
        { requestRender: () => {} },
        plainTheme,
        {
          invocation: { ...inv!, toolName: "custom_plugin_tool" },
          cwd: dir,
          resolveRenderers: () => undefined,
          getRows: () => 24,
        },
        () => {},
      );
      expect(reopened.getMode()).toBe("json");
      clearPreferredToolModes();

      // Custom message renderer helper renders plugin message components cleanly.
      const msgLines = renderPiCustomMessageElement({
        customType: "my-plugin/card",
        message: { type: "custom_message", customType: "my-plugin/card", content: "hello" },
        width: 80,
        theme: plainTheme,
        resolveMessageRenderer: (ct) =>
          ct === "my-plugin/card"
            ? () => ({ invalidate() {}, render: () => ["[PI CUSTOM MSG] card"] })
            : undefined,
      });
      expect(msgLines).toEqual([" [PI CUSTOM MSG] card"]);

      // Tool without custom renderer falls back cleanly to JSON.
      const fallback = createToolRenderTui(
        { requestRender: () => {} },
        plainTheme,
        {
          invocation: { ...inv!, toolName: "unregistered_tool" },
          cwd: dir,
          resolveRenderers: () => undefined,
          getRows: () => 24,
        },
        () => {},
      );
      const fbPlain = fallback.render(width).map(stripAnsi).join("\n");
      expect(fallback.usedJsonFallback()).toBe(true);
      expect(fbPlain).toContain("[Pi Element · fallback]");
      expect(fbPlain).toContain("git status");
      fallback.handleInput("\x1b");
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
