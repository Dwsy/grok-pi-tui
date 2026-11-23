import { describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { SessionPickerTheme } from "../pi-grok-mentions/session-picker-tui.ts";
import {
  aggregateSessionFiles,
  buildEditDiffLines,
  buildReviewFileTree,
  extractSessionReviewTurns,
} from "./review-data.ts";
import { createReviewTui, formatReviewFeedback, type ReviewTuiResult } from "./review-tui.ts";

const plainTheme: SessionPickerTheme = {
  fg: (_color: string, text: string) => text,
  bold: (text: string) => text,
};

function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;]*m/g, "");
}

function makeTempSessionFixture(): { dir: string; jsonl: string } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-review-test-"));
  fs.mkdirSync(path.join(dir, "src"), { recursive: true });
  fs.writeFileSync(
    path.join(dir, "src", "app.ts"),
    "export const version = 2;\nexport const ok = true;\n",
    "utf8",
  );
  fs.writeFileSync(path.join(dir, "README.md"), "# Demo\nUpdated docs.\n", "utf8");

  const jsonl = path.join(dir, "session.jsonl");
  const lines = [
    JSON.stringify({
      type: "message",
      timestamp: "2026-11-23T01:00:00Z",
      message: { role: "user", content: [{ type: "text", text: "Bump app version to 2" }] },
    }),
    JSON.stringify({
      type: "message",
      timestamp: "2026-11-23T01:00:05Z",
      message: {
        role: "assistant",
        content: [
          {
            type: "toolCall",
            name: "edit",
            arguments: {
              path: path.join(dir, "src", "app.ts"),
              oldText: "export const version = 1;\nexport const ok = true;",
              newText: "export const version = 2;\nexport const ok = true;",
            },
          },
        ],
      },
    }),
    JSON.stringify({
      type: "message",
      timestamp: "2026-11-23T01:05:00Z",
      message: { role: "user", content: [{ type: "text", text: "Update README" }] },
    }),
    JSON.stringify({
      type: "message",
      timestamp: "2026-11-23T01:05:04Z",
      message: {
        role: "assistant",
        content: [
          {
            type: "toolCall",
            name: "write",
            arguments: {
              path: "README.md",
              content: "# Demo\nUpdated docs.\n",
            },
          },
        ],
      },
    }),
  ];
  fs.writeFileSync(jsonl, lines.join("\n"), "utf8");
  return { dir, jsonl };
}

describe("review-data extraction and tree building", () => {
  test("extracts turn-grouped edit/write changes and aggregates session files", () => {
    const { dir, jsonl } = makeTempSessionFixture();
    try {
      const turns = extractSessionReviewTurns(jsonl, dir);
      expect(turns).toHaveLength(2);
      expect(turns[0]!.prompt).toBe("Bump app version to 2");
      expect(turns[0]!.files[0]!.relPath).toBe("src/app.ts");
      expect(turns[0]!.files[0]!.additions).toBe(1);
      expect(turns[0]!.files[0]!.deletions).toBe(1);

      const sessionFiles = aggregateSessionFiles(turns);
      expect(sessionFiles.map((f) => f.relPath)).toEqual(["README.md", "src/app.ts"]);

      const tree = buildReviewFileTree(sessionFiles);
      expect(tree.some((r) => r.kind === "dir" && r.name === "src")).toBe(true);
      expect(tree.some((r) => r.kind === "file" && r.name === "app.ts")).toBe(true);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("buildEditDiffLines isolates changed lines with compact context", () => {
    const { lines, additions, deletions } = buildEditDiffLines(
      "a\nb\nc\nd\ne",
      "a\nb\nC_MOD\nd\ne",
    );
    expect(additions).toBe(1);
    expect(deletions).toBe(1);
    expect(lines.some((l) => l.kind === "del" && l.text === "c")).toBe(true);
    expect(lines.some((l) => l.kind === "add" && l.text === "C_MOD")).toBe(true);
  });
});

describe("ReviewTui rendering, simplified keyboard, and mouse interactions", () => {
  test("renders bordered session review with exact width, Diff/File toggle, and note -> Ctrl+S send", () => {
    const { dir, jsonl } = makeTempSessionFixture();
    try {
      const turns = extractSessionReviewTurns(jsonl, dir);
      const files = aggregateSessionFiles(turns);
      let submitted: ReviewTuiResult | undefined;

      const tui = createReviewTui(
        { requestRender: () => {} },
        plainTheme,
        { rootDir: dir, mode: "session", files, turns, nerdIcons: true, getRows: () => 26 },
        (res) => {
          submitted = res;
        },
      );

      const width = 104;
      const lines = tui.render(width);
      for (const line of lines) {
        expect(visibleWidth(line)).toBe(width);
      }
      const plain = lines.map(stripAnsi).join("\n");
      expect(plain).toContain("Review Session (2 files)");
      expect(plain).toContain("[Tree]");
      expect(plain).toContain("[Diff]");
      expect(plain).toContain("README.md");

      // Tab toggles Diff -> File preview.
      tui.handleInput("\t");
      expect(tui.getPreviewMode()).toBe("file");
      expect(tui.render(width).map(stripAnsi).join("\n")).toContain("Updated docs.");

      // Enter opens inline note input, type comment, Enter saves, Ctrl+S submits.
      tui.handleInput("\r");
      tui.handleInput("Add install instructions");
      tui.handleInput("\r");
      expect(tui.getCommentCount()).toBe(1);

      tui.handleInput("\x13"); // Ctrl+S
      expect(submitted?.formattedFeedback).toBe(
        formatReviewFeedback([{ relPath: "README.md", note: "Add install instructions" }]),
      );
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("message mode opens turn picker first and supports mouse click to inspect a turn", () => {
    const { dir, jsonl } = makeTempSessionFixture();
    try {
      const turns = extractSessionReviewTurns(jsonl, dir);
      const files = aggregateSessionFiles(turns);

      const tui = createReviewTui(
        { requestRender: () => {} },
        plainTheme,
        { rootDir: dir, mode: "message", files, turns, nerdIcons: false, getRows: () => 26 },
        () => {},
      );

      expect(tui.isPickingTurn()).toBe(true);
      const lines = tui.render(104);
      const plain = lines.map(stripAnsi).join("\n");
      expect(plain).toContain("Review Message (2 turns)");
      expect(plain).toContain("#1 Bump app version to 2");

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

      // Double-click turn #1 (y=3..4) enters file view for that turn.
      tui.handleMouse(mouse({ type: "click", button: "left", x: 5, y: 3, clickCount: 2 }));
      expect(tui.isPickingTurn()).toBe(false);
      expect(tui.getSelectedFile()?.relPath).toBe("src/app.ts");
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
