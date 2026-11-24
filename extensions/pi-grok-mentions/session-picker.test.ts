import { describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import {
  extractVisibleMessageText,
  filterSessionRecords,
  formatTokensM,
  loadPreviewFromJsonl,
  loadSessionPickerRecords,
  loadSessionPreviewMessages,
  type PreviewMessage,
  type SessionRecord,
} from "./session-data.ts";
import {
  createSessionPickerTui,
  shortModelName,
  type SessionPickerTheme,
} from "./session-picker-tui.ts";

const plainTheme: SessionPickerTheme = {
  fg: (_color: string, text: string) => text,
  bold: (text: string) => text,
};

function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;]*m/g, "");
}

function makeSampleRecords(): SessionRecord[] {
  return [
    {
      id: "01a205e1-a668-72d4-ba0c-efc8d4f6de50",
      shortId: "01a205e1",
      path: "/tmp/s1.jsonl",
      cwd: "/Users/dev/pi-grok-build",
      project: "pi-grok-build",
      title: "修复 @session 动态快捷键与边框布局",
      created: "2026-11-22T09:15:00.000Z",
      modified: "2026-11-23T03:45:00.000Z",
      messageCount: 42,
      userMessages: 14,
      assistantMessages: 28,
      totalTokens: 1_245_000,
      model: "claude-sonnet-4-5",
      searchableMessages: "请帮我实现左右分栏边框布局\n好的，我们用 TS 扩展来渲染会话预览。",
    },
    {
      id: "01a203dd-925f-72d7-abcb-257c596c98ec",
      shortId: "01a203dd",
      path: "/tmp/s2.jsonl",
      cwd: "/Users/dev/other-repo",
      project: "other-repo",
      title: "Database migration checklist",
      created: "2026-11-20T10:00:00.000Z",
      modified: "2026-11-21T12:30:00.000Z",
      messageCount: 18,
      userMessages: 6,
      assistantMessages: 12,
      totalTokens: 84_500,
      model: "gpt-5.4",
      searchableMessages: "Check sqlite WAL mode\nWAL mode is enabled for concurrent readers.",
    },
  ];
}

describe("session-data formatting and filtering", () => {
  test("formats tokens in M units cleanly", () => {
    expect(formatTokensM(0)).toBe("0.00M");
    expect(formatTokensM(4_200)).toBe("0.004M");
    expect(formatTokensM(84_500)).toBe("0.08M");
    expect(formatTokensM(1_245_000)).toBe("1.25M");
    expect(formatTokensM(12_500_000)).toBe("12.5M");
  });

  test("extractVisibleMessageText hides thinking, tool calls, and tool results", () => {
    const content = [
      { type: "thinking", text: "secret chain of thought" },
      { type: "redacted_thinking", text: "hidden reasoning" },
      { type: "toolCall", name: "bash", arguments: { cmd: "ls" } },
      { type: "tool_use", name: "read", input: { path: "a.ts" } },
      { type: "toolResult", text: "raw tool stdout" },
      { type: "text", text: "<think>inline reasoning</think>Visible final answer." },
    ];
    expect(extractVisibleMessageText(content)).toBe("Visible final answer.");
  });

  test("loadPreviewFromJsonl keeps only user & assistant text messages", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-mentions-jsonl-"));
    try {
      const file = path.join(dir, "session.jsonl");
      const lines = [
        JSON.stringify({ type: "session", id: "s1" }),
        JSON.stringify({
          type: "message",
          timestamp: "2026-11-23T01:00:00Z",
          message: { role: "user", content: [{ type: "text", text: "Hello agent" }] },
        }),
        JSON.stringify({
          type: "message",
          timestamp: "2026-11-23T01:00:05Z",
          message: {
            role: "assistant",
            content: [
              { type: "thinking", text: "internal thought should vanish" },
              { type: "toolCall", name: "read", arguments: { path: "foo.rs" } },
              { type: "text", text: "Here is the answer" },
            ],
          },
        }),
        JSON.stringify({
          type: "message",
          timestamp: "2026-11-23T01:00:06Z",
          message: { role: "toolResult", content: [{ type: "text", text: "tool output hidden" }] },
        }),
      ];
      fs.writeFileSync(file, lines.join("\n"), "utf8");

      const preview = loadPreviewFromJsonl(file);
      expect(preview).toHaveLength(2);
      expect(preview[0]!.role).toBe("user");
      expect(preview[0]!.content).toBe("Hello agent");
      expect(preview[1]!.role).toBe("assistant");
      expect(preview[1]!.content).toBe("Here is the answer");
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("SQLite loader joins details cache and excludes thinking/label entries from preview", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-mentions-db-"));
    try {
      const dbPath = path.join(dir, "sessions.db");
      const db = new Database(dbPath);
      db.exec(`
        CREATE TABLE sessions (
          id TEXT PRIMARY KEY,
          path TEXT NOT NULL UNIQUE,
          cwd TEXT NOT NULL,
          name TEXT,
          created TEXT NOT NULL,
          modified TEXT NOT NULL,
          message_count INTEGER NOT NULL,
          first_message TEXT,
          user_messages_text TEXT,
          assistant_messages_text TEXT,
          model TEXT
        );
        CREATE TABLE session_details_cache (
          path TEXT PRIMARY KEY,
          user_messages INTEGER NOT NULL,
          assistant_messages INTEGER NOT NULL,
          input_tokens INTEGER NOT NULL,
          output_tokens INTEGER NOT NULL,
          cache_read_tokens INTEGER NOT NULL,
          cache_write_tokens INTEGER NOT NULL,
          models_json TEXT NOT NULL
        );
        CREATE TABLE message_entries (
          id TEXT PRIMARY KEY,
          entry_id TEXT NOT NULL,
          session_path TEXT NOT NULL,
          role TEXT NOT NULL,
          source_type TEXT NOT NULL,
          content TEXT NOT NULL,
          timestamp TEXT NOT NULL
        );
      `);
      db.query(
        `INSERT INTO sessions VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      ).run(
        "sess-1111-2222",
        "/tmp/sess1.jsonl",
        "/Users/dev/pi-grok-build",
        "SQLite test session",
        "2026-11-22T08:00:00Z",
        "2026-11-23T02:00:00Z",
        10,
        "First user prompt",
        "First user prompt",
        "Assistant reply text",
        "",
      );
      db.query(
        `INSERT INTO session_details_cache VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
      ).run("/tmp/sess1.jsonl", 4, 6, 500_000, 100_000, 900_000, 0, '["anthropic/claude-opus-4"]');
      db.query(`INSERT INTO message_entries VALUES (?, ?, ?, ?, ?, ?, ?)`).run(
        "m1",
        "e1",
        "/tmp/sess1.jsonl",
        "user",
        "user",
        "First user prompt",
        "2026-11-22T08:00:01Z",
      );
      db.query(`INSERT INTO message_entries VALUES (?, ?, ?, ?, ?, ?, ?)`).run(
        "m2",
        "e2",
        "/tmp/sess1.jsonl",
        "assistant",
        "thinking",
        "Secret reasoning must not show",
        "2026-11-22T08:00:02Z",
      );
      db.query(`INSERT INTO message_entries VALUES (?, ?, ?, ?, ?, ?, ?)`).run(
        "m3",
        "e3",
        "/tmp/sess1.jsonl",
        "assistant",
        "assistant",
        "Assistant reply text",
        "2026-11-22T08:00:03Z",
      );
      db.close();

      const records = loadSessionPickerRecords({ dbPath });
      expect(records).toHaveLength(1);
      expect(records[0]!.totalTokens).toBe(1_500_000);
      expect(records[0]!.model).toBe("anthropic/claude-opus-4");
      expect(records[0]!.userMessages).toBe(4);
      expect(records[0]!.assistantMessages).toBe(6);

      const preview = loadSessionPreviewMessages("/tmp/sess1.jsonl", { dbPath });
      expect(preview).toHaveLength(2);
      expect(preview.map((m) => m.content)).toEqual(["First user prompt", "Assistant reply text"]);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("filterSessionRecords defaults to global, filters project scope, and searches user/agent messages", () => {
    const records = makeSampleRecords();
    const globalAll = filterSessionRecords(records, {
      scope: "global",
      currentCwd: "/Users/dev/pi-grok-build",
      query: "",
    });
    expect(globalAll).toHaveLength(2);

    const projectOnly = filterSessionRecords(records, {
      scope: "project",
      currentCwd: "/Users/dev/pi-grok-build",
      query: "",
    });
    expect(projectOnly).toHaveLength(1);
    expect(projectOnly[0]!.record.project).toBe("pi-grok-build");

    const msgMatch = filterSessionRecords(records, {
      scope: "global",
      currentCwd: "/Users/dev/pi-grok-build",
      query: "WAL concurrent",
    });
    expect(msgMatch).toHaveLength(1);
    expect(msgMatch[0]!.record.id).toBe("01a203dd-925f-72d7-abcb-257c596c98ec");
    expect(msgMatch[0]!.matchSnippet).toContain("WAL");
  });
});

describe("SessionPickerTui rendering, keyboard, and mouse interactions", () => {
  const previewMap = new Map<string, PreviewMessage[]>([
    [
      "/tmp/s1.jsonl",
      Array.from({ length: 12 }, (_, i) => ({
        role: i % 2 === 0 ? ("user" as const) : ("assistant" as const),
        content: `Line ${i + 1}: 会话预览内容 testing wrap and scrolling`,
        timestamp: "2026-11-23T03:00:00.000Z",
      })),
    ],
    [
      "/tmp/s2.jsonl",
      [
        { role: "user", content: "Check sqlite WAL mode", timestamp: "2026-11-21T12:00:00.000Z" },
        { role: "assistant", content: "WAL mode is enabled.", timestamp: "2026-11-21T12:01:00.000Z" },
      ],
    ],
  ]);

  test("renders full bordered left/right layout with exact display width and all metadata fields", () => {
    const picker = createSessionPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleRecords(),
        currentCwd: "/Users/dev/pi-grok-build",
        loadPreview: (p) => previewMap.get(p) ?? [],
        getRows: () => 28,
      },
      () => {},
    );

    const width = 104;
    const lines = picker.render(width);
    expect(lines.length).toBeGreaterThanOrEqual(16);

    // Every line must match exact terminal column width so borders align.
    for (const line of lines) {
      expect(visibleWidth(line)).toBe(width);
    }

    const plain = lines.map(stripAnsi).join("\n");
    expect(lines[0]!.startsWith("╭")).toBe(true);
    expect(lines[0]!.endsWith("╮")).toBe(true);
    expect(lines[lines.length - 1]!.startsWith("╰")).toBe(true);
    expect(lines[lines.length - 1]!.endsWith("╯")).toBe(true);
    expect(plain).toContain("┬");
    expect(plain).toContain("┴");

    // Required metadata fields on right pane:
    expect(plain).toContain("[Global 2]");
    expect(plain).toContain("[Project 1]");
    expect(plain).toContain("Project: pi-grok-build");
    expect(plain).toContain("ID: 01a205e1");
    expect(plain).toContain("Created:");
    expect(plain).toContain("Updated:");
    expect(plain).toContain("Msgs: 42 (14U/28A)");
    expect(plain).toContain("Tokens: 1.25M");
    expect(plain).toContain("Model: claude-sonnet-4-5");
    expect(plain).toContain("You");
    expect(plain).toContain("Agent");
  });

  test("supports keyboard search, Tab scope toggle, navigation, and Enter confirm", () => {
    let resolved: SessionRecord | undefined;
    const picker = createSessionPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleRecords(),
        currentCwd: "/Users/dev/pi-grok-build",
        loadPreview: (p) => previewMap.get(p) ?? [],
        getRows: () => 28,
      },
      (sel) => {
        resolved = sel;
      },
    );

    // Default scope is global.
    expect(picker.getScope()).toBe("global");
    picker.render(100);

    // Tab switches to project scope.
    picker.handleInput("\t");
    expect(picker.getScope()).toBe("project");

    // Tab switches back to global.
    picker.handleInput("\t");
    expect(picker.getScope()).toBe("global");

    // Typing filters to the second session by message text.
    picker.handleInput("WAL");
    expect(picker.getQuery()).toBe("WAL");
    expect(picker.getSelectedRecord()?.shortId).toBe("01a203dd");

    // Enter confirms selection.
    picker.handleInput("\r");
    expect(resolved?.id).toBe("01a203dd-925f-72d7-abcb-257c596c98ec");
  });

  test("supports mouse click on scope pills, left row selection, double-click confirm, and wheel scrolling", () => {
    let resolved: SessionRecord | undefined;
    let closed = false;
    const picker = createSessionPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleRecords(),
        currentCwd: "/Users/dev/pi-grok-build",
        loadPreview: (p) => previewMap.get(p) ?? [],
        getRows: () => 28,
      },
      (sel) => {
        closed = true;
        resolved = sel;
      },
    );

    const lines = picker.render(104);
    const headerPlain = stripAnsi(lines[1]!);
    const projectCol = headerPlain.indexOf("[Project 1]");
    const globalCol = headerPlain.indexOf("[Global 2]");
    expect(projectCol).toBeGreaterThan(0);

    const mouse = (overrides: Partial<TuiMouseEvent>): TuiMouseEvent => ({
      type: "press",
      button: "left",
      x: 0,
      y: 0,
      screenX: 0,
      screenY: 0,
      width: 104,
      height: lines.length,
      shift: false,
      alt: false,
      ctrl: false,
      ...overrides,
    });

    // Click [Project 1] pill.
    picker.handleMouse(mouse({ x: projectCol + 2, y: 1 }));
    expect(picker.getScope()).toBe("project");

    // Click [Global 2] pill.
    picker.handleMouse(mouse({ x: globalCol + 2, y: 1 }));
    expect(picker.getScope()).toBe("global");
    picker.render(104);

    // Click second session row in left pane (bodyStartY=5; row 0 is y=5..6, row 1 is y=7..8).
    picker.handleMouse(mouse({ x: 5, y: 7 }));
    expect(picker.getSelectedRecord()?.shortId).toBe("01a203dd");

    // Wheel up on left pane moves back to first session.
    picker.handleMouse(mouse({ type: "wheel", button: "none", x: 5, y: 6, wheelDelta: -1 }));
    expect(picker.getSelectedRecord()?.shortId).toBe("01a205e1");

    // Wheel down on right pane scrolls preview without changing selection.
    const beforePreview = picker.render(104).join("\n");
    picker.handleMouse(mouse({ type: "wheel", button: "none", x: 60, y: 10, wheelDelta: 1 }));
    const afterPreview = picker.render(104).join("\n");
    expect(picker.getSelectedRecord()?.shortId).toBe("01a205e1");
    expect(afterPreview).not.toBe(beforePreview);

    // Double-click on row 1 confirms the second session.
    picker.handleMouse(mouse({ type: "click", button: "left", x: 5, y: 7, clickCount: 2 }));
    expect(closed).toBe(true);
    expect(resolved?.shortId).toBe("01a203dd");
  });

  test("fullscreen Codex-style /resume layout fills terminal dimensions, groups by project, and handles mouse + keyboard", () => {
    let resolved: SessionRecord | undefined;
    const picker = createSessionPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleRecords(),
        currentCwd: "/Users/dev/pi-grok-build",
        titleBadge: "(/resume)",
        confirmLabel: "resume",
        fullscreen: true,
        loadPreview: (p) => previewMap.get(p) ?? [],
        getRows: () => 30,
      },
      (sel) => {
        resolved = sel;
      },
    );

    const width = 120;
    const lines = picker.render(width);
    expect(lines.length).toBe(30);
    for (const line of lines) {
      expect(visibleWidth(line)).toBe(width);
    }

    const plain = lines.map(stripAnsi).join("\n");
    // Edge-to-edge layout without outer box border.
    expect(lines[0]!.startsWith("╭")).toBe(false);
    expect(plain).toContain("Group: Project");
    expect(plain).toContain("All 2");
    expect(plain).toContain("Project 1");
    expect(plain).toContain("Session details");
    expect(plain).toContain("Last messages");
    expect(plain).toContain("Msgs");
    expect(plain).toContain("Created:");
    expect(plain).toContain("Updated:");
    expect(plain).toContain("Messages: 42 (14U/28A)");
    expect(shortModelName("magpie/auto/deepseek-v4.1-flash")).toBe("deepseek-v4.1-flash");
    expect(shortModelName("gpt-5.4")).toBe("gpt-5.4");
    expect(plain).toContain("/Users/dev/pi-grok-build");
    expect(plain).toContain("/Users/dev/other-repo");

    // Keyboard navigation skips group headers and selects second session.
    expect(picker.getSelectedRecord()?.shortId).toBe("01a205e1");
    picker.handleInput("\x1b[B"); // Down arrow
    expect(picker.getSelectedRecord()?.shortId).toBe("01a203dd");

    // Mouse click on `Project 1` tab switches scope to project.
    const tabRow = stripAnsi(lines[1]!);
    const projX = tabRow.indexOf("Project 1");
    expect(projX).toBeGreaterThan(0);
    picker.handleMouse({
      type: "press",
      button: "left",
      x: projX + 2,
      y: 1,
      screenX: projX + 2,
      screenY: 1,
      width,
      height: lines.length,
      shift: false,
      alt: false,
      ctrl: false,
    });
    expect(picker.getScope()).toBe("project");
    expect(picker.getSelectedRecord()?.shortId).toBe("01a205e1");

    // Enter confirms resume.
    picker.handleInput("\r");
    expect(resolved?.id).toBe("01a205e1-a668-72d4-ba0c-efc8d4f6de50");
  });
});
