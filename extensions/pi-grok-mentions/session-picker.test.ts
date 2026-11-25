import { describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import {
  appendSessionNameToJsonl,
  deleteSessionJsonl,
  extractVisibleMessageText,
  filterSessionRecords,
  findSessionRecordById,
  formatTokensM,
  loadPreviewFromJsonl,
  loadSessionPickerRecords,
  loadSessionPreviewMessages,
  loadSessionRecordDetails,
  searchSessionPickerRecords,
  type PreviewMessage,
  type SessionRecord,
} from "./session-data.ts";
import {
  defaultLiteDbPath,
  resetLiteSyncCooldown,
  scheduleLiteDbBackgroundSync,
  syncSessionLiteDb,
} from "./session-lite-db.ts";
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

      // Large (>256KB) JSONL file uses tail-chunked reading and returns trailing messages cleanly.
      const largeFile = path.join(dir, "large-session.jsonl");
      const filler = "a".repeat(4000);
      const largeLines = [
        JSON.stringify({ type: "session", id: "s-large" }),
        ...Array.from({ length: 90 }, (_, i) =>
          JSON.stringify({
            type: "message",
            timestamp: `2026-11-23T01:${String(i % 60).padStart(2, "0")}:00Z`,
            message: { role: i % 2 === 0 ? "user" : "assistant", content: `msg-${i} ${filler}` },
          }),
        ),
      ];
      fs.writeFileSync(largeFile, largeLines.join("\n") + "\n", "utf8");
      const tailPreview = loadPreviewFromJsonl(largeFile, 5);
      expect(tailPreview).toHaveLength(5);
      expect(tailPreview[0]!.content.startsWith("msg-85 ")).toBe(true);
      expect(tailPreview[4]!.content.startsWith("msg-89 ")).toBe(true);
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

  test("fullscreen session rows clip long titles without an ellipsis", () => {
    const records = makeSampleRecords();
    records[0]!.title = "这是一条特别特别长的会话标题，用来验证列宽截断后不会追加省略号".repeat(3);
    const picker = createSessionPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records,
        currentCwd: "/Users/dev/pi-grok-build",
        titleBadge: "(/resume)",
        fullscreen: true,
        getRows: () => 16,
      },
      () => {},
    );

    const titleLine = picker
      .render(90)
      .map(stripAnsi)
      .find((line) => line.includes("这是一条特别"));
    expect(titleLine).toBeDefined();
    expect(titleLine).not.toContain("…");
  });

  test("debounces session search, shows searching state, and flushes on Enter", async () => {
    const dbQueries: string[] = [];
    let renderRequests = 0;
    const picker = createSessionPickerTui(
      { requestRender: () => { renderRequests += 1; } },
      plainTheme,
      {
        records: makeSampleRecords(),
        currentCwd: "/Users/dev/pi-grok-build",
        titleBadge: "(/resume)",
        fullscreen: true,
        searchDebounceMs: 35,
        searchRecords: ({ query }) => {
          dbQueries.push(query);
          return [];
        },
        getRows: () => 20,
      },
      () => {},
    );

    picker.handleInput("m");
    picker.handleInput("i");
    picker.handleInput("g");
    expect(dbQueries).toEqual([]);
    expect(picker.render(100).map(stripAnsi).join("\n")).toContain("searching");

    await new Promise((r) => setTimeout(r, 60));
    expect(dbQueries).toEqual(["mig"]);
    expect(renderRequests).toBeGreaterThanOrEqual(1);
    expect(picker.render(100).map(stripAnsi).join("\n")).not.toContain("searching");

    picker.handleInput("x");
    picker.handleInput("\x1b");
    await new Promise((r) => setTimeout(r, 60));
    expect(dbQueries).toEqual(["mig"]);

    picker.handleInput("x");
    picker.handleInput("\r");
    expect(dbQueries).toEqual(["mig", "migxx"]);
  });

  test("JSONL helpers append session_info and handle missing files", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-mentions-jsonl-"));
    try {
      const file = path.join(dir, "session.jsonl");
      fs.writeFileSync(file, '{"type":"session","id":"s1"}\n', "utf8");
      expect(appendSessionNameToJsonl(file, "My session")).toEqual({ ok: true });
      const lines = fs.readFileSync(file, "utf8").trim().split("\n");
      const info = JSON.parse(lines[lines.length - 1]!);
      expect(info.type).toBe("session_info");
      expect(info.name).toBe("My session");
      expect(info.parentId).toBeNull();
      expect(appendSessionNameToJsonl(file, "  ")).toMatchObject({ ok: false });

      // Missing trailing newline must not corrupt the previous JSONL line, and rapid appends get unique IDs.
      const noTrailingNewline = path.join(dir, "no-nl.jsonl");
      fs.writeFileSync(noTrailingNewline, '{"type":"session","id":"s2"}', "utf8");
      expect(appendSessionNameToJsonl(noTrailingNewline, "First name")).toEqual({ ok: true });
      expect(appendSessionNameToJsonl(noTrailingNewline, "Second name")).toEqual({ ok: true });
      const parsed = fs
        .readFileSync(noTrailingNewline, "utf8")
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line) as { type: string; id: string; name?: string });
      expect(parsed).toHaveLength(3);
      expect(parsed[0]!.id).toBe("s2");
      expect(parsed[1]!.name).toBe("First name");
      expect(parsed[2]!.name).toBe("Second name");
      expect(parsed[1]!.id).not.toBe(parsed[2]!.id);

      expect(deleteSessionJsonl(path.join(dir, "missing.jsonl"))).toEqual({
        ok: true,
        method: "missing",
      });
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("SQLite loader has no 300-row cap, supports fast slim load, lazy details, and findSessionRecordById", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-mentions-many-"));
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
      `);
      const insertSession = db.query(
        `INSERT INTO sessions VALUES (?, ?, ?, ?, ?, ?, ?, ?, '', '', ?)`,
      );
      const insertDetail = db.query(
        `INSERT INTO session_details_cache VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
      );
      for (let i = 0; i < 350; i++) {
        const num = String(i).padStart(4, "0");
        const ts = `2026-11-${String(1 + (i % 25)).padStart(2, "0")}T${String(i % 24).padStart(2, "0")}:${String(i % 60).padStart(2, "0")}:00Z`;
        insertSession.run(
          `sess-${num}-abcd`,
          `/tmp/sess-${num}.jsonl`,
          i % 2 === 0 ? "/Users/dev/pi-grok-build" : "/Users/dev/other-repo",
          i === 0 ? "Oldest named session" : null,
          ts,
          ts,
          i + 1,
          `Prompt ${num}`,
          "",
        );
      }
      insertDetail.run("/tmp/sess-0000.jsonl", 5, 7, 100_000, 200_000, 300_000, 0, '["gpt-5.4"]');
      db.close();

      const allSlim = loadSessionPickerRecords({ dbPath, includeDetails: false });
      expect(allSlim).toHaveLength(350);
      const target = allSlim.find((r) => r.id === "sess-0000-abcd")!;
      expect(target).toBeDefined();
      expect(target.userMessages).toBeNull();

      const details = loadSessionRecordDetails(target.path, { dbPath });
      expect(details).toEqual({
        userMessages: 5,
        assistantMessages: 7,
        totalTokens: 600_000,
        model: "gpt-5.4",
      });

      const byPrefix = findSessionRecordById("sess-0000", { dbPath });
      expect(byPrefix?.id).toBe("sess-0000-abcd");
      expect(byPrefix?.totalTokens).toBe(600_000);
      expect(byPrefix?.model).toBe("gpt-5.4");

      // SessionPickerTui lazily hydrates details on render for the selected slim record.
      const picker = createSessionPickerTui(
        { requestRender: () => {} },
        plainTheme,
        {
          records: [target],
          currentCwd: "/Users/dev/pi-grok-build",
          fullscreen: true,
          loadPreview: () => [],
          loadDetails: (p) => loadSessionRecordDetails(p, { dbPath }),
          getRows: () => 24,
        },
        () => {},
      );
      const rendered = picker.render(110).map(stripAnsi).join("\n");
      expect(rendered).toContain("Messages: 1 (5U/7A)");
      expect(rendered).toContain("0.60M");
      expect(rendered).toContain("Model: gpt-5.4");
      picker.handleInput("\x1b");
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("named filter keeps only sessions with a JSONL name", () => {
    const records = makeSampleRecords();
    records[1]!.name = "Named migration";
    const named = filterSessionRecords(records, {
      scope: "global",
      currentCwd: "/Users/dev/pi-grok-build",
      query: "",
      nameFilter: "named",
    });
    expect(named.map((h) => h.record.shortId)).toEqual(["01a203dd"]);
  });

  test("Ctrl+S cycles sort, Ctrl+N filters named sessions, Ctrl+P shows path", () => {
    const records = makeSampleRecords();
    records[0]!.name = "Alpha";
    const picker = createSessionPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records,
        currentCwd: "/Users/dev/pi-grok-build",
        titleBadge: "(/resume)",
        fullscreen: true,
        getRows: () => 24,
      },
      () => {},
    );

    const plain = () => picker.render(110).map(stripAnsi).join("\n");
    const groupLineCount = () =>
      plain()
        .split("\n")
        .filter((line) => /^  \/Users\/dev\/\S+  \d+/.test(line)).length;

    expect(groupLineCount()).toBe(2);
    picker.handleInput("\x13"); // ctrl+s -> recent
    const recentPlain = plain();
    expect(recentPlain).toContain("Sort: Recent");
    expect(recentPlain).not.toContain("Group: Project");
    expect(recentPlain).toContain("修复 @session");
    expect(recentPlain).toContain("Database migration");
    expect(groupLineCount()).toBe(0);
    picker.handleInput("\x13"); // -> relevance
    const relevancePlain = plain();
    expect(relevancePlain).toContain("Sort: Relevance");
    expect(relevancePlain).not.toContain("Group: Project");
    expect(relevancePlain).toContain("修复 @session");
    expect(relevancePlain).toContain("Database migration");
    picker.handleInput("\x13"); // -> threaded
    expect(plain()).toContain("Sort: Threaded");
    expect(groupLineCount()).toBe(2);

    picker.handleInput("\x0e"); // ctrl+n -> named
    expect(picker.getSelectedRecord()?.shortId).toBe("01a205e1");
    expect(plain()).toContain("Name: Named");
    expect(plain()).toContain("1/2");

    picker.handleInput("\x10"); // ctrl+p -> path on
    expect(plain()).toContain("path (on)");
    expect(plain()).toContain("Path");
    expect(plain()).toContain("/tmp/s1.jsonl");

    picker.handleInput("\x1b");
  });

  test("Ctrl+D deletes after confirmation and refuses the active session", () => {
    const deleted: string[] = [];
    const picker = createSessionPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleRecords(),
        currentCwd: "/Users/dev/pi-grok-build",
        fullscreen: true,
        deleteSession: (p) => {
          deleted.push(p);
          return { ok: true };
        },
        getRows: () => 24,
      },
      () => {},
    );

    picker.handleInput("\x04"); // ctrl+d
    expect(deleted).toEqual([]);
    expect(picker.render(110).map(stripAnsi).join("\n")).toContain("Delete session?");
    picker.handleInput("\r");
    expect(deleted).toEqual(["/tmp/s1.jsonl"]);
    expect(picker.getSelectedRecord()?.shortId).toBe("01a203dd");
    picker.handleInput("\x1b");

    const activeDeleted: string[] = [];
    const active = createSessionPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleRecords(),
        currentCwd: "/Users/dev/pi-grok-build",
        currentSessionPath: "/tmp/s1.jsonl",
        fullscreen: true,
        deleteSession: (p) => {
          activeDeleted.push(p);
          return { ok: true };
        },
        getRows: () => 24,
      },
      () => {},
    );
    active.handleInput("\x04");
    expect(activeDeleted).toEqual([]);
    expect(active.render(110).map(stripAnsi).join("\n")).toContain(
      "Cannot delete the currently active session",
    );
    active.handleInput("\x1b");
  });

  test("Ctrl+R renames selected session into JSONL via injected callback", () => {
    const renamed: string[] = [];
    const picker = createSessionPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleRecords(),
        currentCwd: "/Users/dev/pi-grok-build",
        fullscreen: true,
        renameSession: (p, name) => {
          renamed.push(`${p}:${name}`);
          return { ok: true };
        },
        getRows: () => 24,
      },
      () => {},
    );

    picker.handleInput("\x12"); // ctrl+r
    expect(picker.render(110).map(stripAnsi).join("\n")).toContain("Rename Session");
    for (const ch of "My session") picker.handleInput(ch);
    picker.handleInput("\r");
    expect(renamed).toEqual(["/tmp/s1.jsonl:My session"]);
    expect(picker.getSelectedRecord()?.title).toBe("My session");
    expect(picker.render(110).map(stripAnsi).join("\n")).toContain("Name: My session");
    picker.handleInput("\x1b");
  });

  test("rapid wheel scrolling coalesces preview renders without reloading messages", async () => {
    let loadCalls = 0;
    let renderRequests = 0;
    const picker = createSessionPickerTui(
      { requestRender: () => { renderRequests += 1; } },
      plainTheme,
      {
        records: makeSampleRecords(),
        currentCwd: "/Users/dev/pi-grok-build",
        fullscreen: true,
        loadPreview: () => {
          loadCalls += 1;
          return previewMap.get("/tmp/s1.jsonl") ?? [];
        },
        getRows: () => 20,
      },
      () => {},
    );
    picker.render(100);
    expect(loadCalls).toBe(1);
    const before = renderRequests;

    for (let i = 0; i < 8; i++) {
      picker.handleMouse({
        type: "wheel",
        wheelDelta: -1,
        button: "left",
        x: 80,
        y: 8,
        screenX: 80,
        screenY: 8,
        width: 100,
        height: 20,
        shift: false,
        alt: false,
        ctrl: false,
      });
    }
    expect(renderRequests).toBe(before);
    expect(loadCalls).toBe(1);
    await new Promise((r) => setTimeout(r, 70));
    expect(renderRequests).toBeGreaterThanOrEqual(before + 1);
    expect(loadCalls).toBe(1);
    picker.handleInput("\x1b");
  });

  test("session details renders message text through injected Pi markdown renderer", () => {
    const rendered: string[] = [];
    const picker = createSessionPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleRecords(),
        currentCwd: "/Users/dev/pi-grok-build",
        fullscreen: true,
        loadPreview: () => previewMap.get("/tmp/s1.jsonl") ?? [],
        markdownRenderer: (text, _width) => {
          rendered.push(text);
          return [`MD|${text.slice(0, 12)}`];
        },
        getRows: () => 30,
      },
      () => {},
    );

    const plain = picker.render(100).map(stripAnsi).join("\n");
    expect(rendered.some((text) => text.includes("会话预览内容"))).toBe(true);
    expect(plain).toContain("MD|");
    picker.handleInput("\x1b");
  });

  test("resume divider drag adjusts the split in memory only", () => {
    const width = 100;
    const makePicker = () =>
      createSessionPickerTui(
        { requestRender: () => {} },
        plainTheme,
        {
          records: makeSampleRecords(),
          currentCwd: "/Users/dev/pi-grok-build",
          titleBadge: "(/resume)",
          fullscreen: true,
          getRows: () => 20,
        },
        () => {},
      );
    const dividerColumn = (picker: ReturnType<typeof createSessionPickerTui>) => {
      const line = stripAnsi(picker.render(width)[3]!);
      return line.indexOf("│");
    };
    const mouse = (
      type: TuiMouseEvent["type"],
      x: number,
    ): TuiMouseEvent => ({
      type,
      button: type === "release" ? "none" : "left",
      x,
      y: 8,
      screenX: x,
      screenY: 8,
      width,
      height: 20,
      shift: false,
      alt: false,
      ctrl: false,
    });

    const picker = makePicker();
    expect(dividerColumn(picker)).toBe(63);

    expect(picker.handleMouse(mouse("press", 63))?.capture).toBe(true);
    picker.handleMouse(mouse("drag", 45));
    expect(dividerColumn(picker)).toBe(45);
    picker.handleMouse(mouse("drag", 5));
    expect(dividerColumn(picker)).toBe(20);
    picker.handleMouse(mouse("drag", 200));
    expect(dividerColumn(picker)).toBe(71);
    picker.handleMouse(mouse("release", 71));

    expect(dividerColumn(makePicker())).toBe(63);
  });

  test("resume hides the Model column when the left pane is narrow", () => {
    const makePicker = () =>
      createSessionPickerTui(
        { requestRender: () => {} },
        plainTheme,
        {
          records: makeSampleRecords(),
          currentCwd: "/Users/dev/pi-grok-build",
          titleBadge: "(/resume)",
          fullscreen: true,
          getRows: () => 20,
        },
        () => {},
      );
    const headerLine = (width: number) =>
      stripAnsi(makePicker().render(width)[3]!);

    expect(headerLine(90)).not.toContain("Model");
    expect(headerLine(120)).toContain("Model");
  });

  test("resume session details header stays compact", () => {
    const picker = createSessionPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleRecords(),
        currentCwd: "/Users/dev/pi-grok-build",
        titleBadge: "(/resume)",
        fullscreen: true,
        getRows: () => 20,
      },
      () => {},
    );
    const lines = picker.render(110).map(stripAnsi);
    expect(lines.findIndex((line) => line.includes("Last messages"))).toBeLessThanOrEqual(10);
  });
});

describe("sessions-lite.db fallback when PSM sessions.db is absent", () => {
  test("lazily indexes JSONL files, probes head+tail on large files, hydrates details, and syncs incrementally", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-mentions-lite-"));
    try {
      const projDir = path.join(dir, "--Users-dev-demo--");
      fs.mkdirSync(projDir, { recursive: true });

      const smallPath = path.join(projDir, "2026-11-24_small1111.jsonl");
      fs.writeFileSync(
        smallPath,
        [
          JSON.stringify({
            type: "session",
            id: "lite-small-1111",
            cwd: "/Users/dev/demo",
            timestamp: "2026-11-24T08:00:00.000Z",
          }),
          JSON.stringify({
            type: "message",
            timestamp: "2026-11-24T08:01:00.000Z",
            message: { role: "user", content: [{ type: "text", text: "Fix SQLite WAL checkpoint" }] },
          }),
          JSON.stringify({
            type: "message",
            timestamp: "2026-11-24T08:02:00.000Z",
            message: {
              role: "assistant",
              provider: "anthropic",
              model: "claude-sonnet-4-5",
              content: [{ type: "text", text: "Done, enabled WAL autocheckpoint." }],
              usage: { input: 1200, output: 300, cacheRead: 500, cacheWrite: 0 },
            },
          }),
        ].join("\n") + "\n",
        "utf8",
      );

      // Large > 64KB JSONL file to exercise bounded head+tail probe + tail session_info rename.
      const largePath = path.join(projDir, "2026-11-24_large2222.jsonl");
      const filler = "x".repeat(4000);
      const largeLines = [
        JSON.stringify({
          type: "session",
          id: "lite-large-2222",
          cwd: "/Users/dev/other",
          timestamp: "2026-11-23T09:00:00.000Z",
        }),
        JSON.stringify({
          type: "message",
          timestamp: "2026-11-23T09:01:00.000Z",
          message: { role: "user", content: "Initial large prompt" },
        }),
        ...Array.from({ length: 20 }, (_, i) =>
          JSON.stringify({
            type: "message",
            timestamp: `2026-11-23T09:${String(2 + i).padStart(2, "0")}:00.000Z`,
            message: {
              role: "assistant",
              model: "gpt-5.4",
              content: [{ type: "text", text: `chunk-${i} ${filler}` }],
              usage: { input: 100, output: 50 },
            },
          }),
        ),
        JSON.stringify({
          type: "session_info",
          id: "session-info-tail",
          parentId: null,
          timestamp: "2026-11-24T10:00:00.000Z",
          name: "Tail Renamed Session",
        }),
      ];
      fs.writeFileSync(largePath, largeLines.join("\n") + "\n", "utf8");

      // 1. No sessions.db -> auto-creates sessions-lite.db and loads all sessions.
      const records = loadSessionPickerRecords({ sessionsDir: dir, includeDetails: false });
      expect(fs.existsSync(defaultLiteDbPath(dir))).toBe(true);
      expect(records).toHaveLength(2);
      const largeRec = records.find((r) => r.id === "lite-large-2222")!;
      const smallRec = records.find((r) => r.id === "lite-small-1111")!;
      expect(largeRec.title).toBe("Tail Renamed Session");
      expect(largeRec.model).toBe("gpt-5.4");
      expect(smallRec.title).toBe("Fix SQLite WAL checkpoint");
      expect(smallRec.model).toBe("anthropic/claude-sonnet-4-5");

      // 2. Second forced sync skips unchanged files (0 scanned, 2 skipped).
      const sync2 = syncSessionLiteDb({ sessionsDir: dir, force: true });
      expect(sync2.usedSqlite).toBe(true);
      expect(sync2.scanned).toBe(0);
      expect(sync2.skipped).toBe(2);
      expect(sync2.removed).toBe(0);

      // 3. Lazy detail hydration computes exact counts/tokens and caches them.
      const details = loadSessionRecordDetails(smallPath, { sessionsDir: dir });
      expect(details).toEqual({
        userMessages: 1,
        assistantMessages: 1,
        totalTokens: 2000,
        model: "anthropic/claude-sonnet-4-5",
      });

      // 4. Search and ID prefix lookup work on sessions-lite.db.
      const hits = searchSessionPickerRecords({
        query: "WAL checkpoint",
        scope: "global",
        currentCwd: "/Users/dev/demo",
        sessionsDir: dir,
      });
      expect(hits.map((h) => h.record.id)).toEqual(["lite-small-1111"]);
      expect(findSessionRecordById("lite-large", { sessionsDir: dir })?.id).toBe("lite-large-2222");

      // 5. Incremental sync picks up modified + deleted files only.
      fs.appendFileSync(
        smallPath,
        JSON.stringify({
          type: "session_info",
          id: "session-info-new",
          parentId: null,
          timestamp: "2026-11-24T11:00:00.000Z",
          name: "Updated Small Title",
        }) + "\n",
        "utf8",
      );
      fs.unlinkSync(largePath);
      resetLiteSyncCooldown(defaultLiteDbPath(dir));
      const sync3 = syncSessionLiteDb({ sessionsDir: dir, force: true });
      expect(sync3.scanned).toBe(1);
      expect(sync3.skipped).toBe(0);
      expect(sync3.removed).toBe(1);
      const after = loadSessionPickerRecords({ sessionsDir: dir, includeDetails: false });
      expect(after).toHaveLength(1);
      expect(after[0]!.title).toBe("Updated Small Title");

      // 6. Corrupt sessions-lite.db self-recovers cleanly.
      fs.writeFileSync(defaultLiteDbPath(dir), "not a valid sqlite file", "utf8");
      resetLiteSyncCooldown(defaultLiteDbPath(dir));
      const recovered = syncSessionLiteDb({ sessionsDir: dir, force: true });
      expect(recovered.usedSqlite).toBe(true);
      expect(recovered.scanned).toBe(1);

      // 7. Non-blocking background sync schedules cleanly when sessions-lite.db exists.
      const scheduled = scheduleLiteDbBackgroundSync({ sessionsDir: dir });
      expect(scheduled).toBe(true);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("never creates sessions-lite.db when PSM sessions.db is present", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-mentions-psm-prio-"));
    try {
      const psmDbPath = path.join(dir, "sessions.db");
      const db = new Database(psmDbPath);
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
      `);
      db.query(`INSERT INTO sessions VALUES (?, ?, ?, ?, ?, ?, ?, ?, '', '', ?)`).run(
        "psm-only-1",
        "/tmp/psm-only-1.jsonl",
        "/Users/dev/demo",
        "From PSM DB",
        "2026-11-24T08:00:00Z",
        "2026-11-24T08:00:00Z",
        3,
        "Hello",
        "gpt-5.4",
      );
      db.close();

      const records = loadSessionPickerRecords({ sessionsDir: dir, includeDetails: false });
      expect(records).toHaveLength(1);
      expect(records[0]!.title).toBe("From PSM DB");
      expect(fs.existsSync(defaultLiteDbPath(dir))).toBe(false);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
