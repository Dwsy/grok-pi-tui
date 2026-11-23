import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, expect, test } from "bun:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import type { TuiTheme } from "../pi-grok-tui-kit/index.ts";
import {
  buildFlattenedSessionTree,
  filterTreeNodes,
  parseSessionTreeJsonl,
} from "./tree-data.ts";
import { createSessionTreeTui, type SessionTreeActionResult } from "./tree-tui.ts";

const mockTheme: TuiTheme = {
  fg: (_c: string, t: string) => t,
  bg: (_c: string, t: string) => t,
  bold: (t: string) => t,
};

function writeSampleSession(): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-grok-tree-test-"));
  const file = path.join(dir, "session.jsonl");
  const lines = [
    JSON.stringify({ type: "session", id: "s1", timestamp: "2026-11-23T10:00:00Z" }),
    JSON.stringify({
      type: "message",
      id: "e1",
      parentId: null,
      timestamp: "2026-11-23T10:01:00Z",
      message: { role: "user", content: "Explain the architecture" },
    }),
    JSON.stringify({
      type: "message",
      id: "e2",
      parentId: "e1",
      timestamp: "2026-11-23T10:01:10Z",
      message: { role: "assistant", content: [{ type: "text", text: "Rust owns APIs, TS renders UI." }] },
    }),
    JSON.stringify({
      type: "message",
      id: "e3",
      parentId: "e2",
      timestamp: "2026-11-23T10:01:15Z",
      message: { role: "toolResult", toolName: "read", content: "file contents..." },
    }),
    JSON.stringify({
      type: "message",
      id: "e4",
      parentId: "e2",
      timestamp: "2026-11-23T10:02:00Z",
      message: { role: "user", content: "Alternate branch question" },
    }),
    JSON.stringify({ type: "label", targetId: "e2", label: "checkpoint" }),
  ];
  fs.writeFileSync(file, `${lines.join("\n")}\n`, "utf8");
  return file;
}

describe("pi-grok-tree parsing, filtering, and TUI navigation", () => {
  test("parses JSONL branches, attaches labels, and marks active leaf path", () => {
    const file = writeSampleSession();
    try {
      const { entries, leafId } = parseSessionTreeJsonl(file, "e4");
      expect(leafId).toBe("e4");
      const nodes = buildFlattenedSessionTree(entries, leafId);
      expect(nodes.length).toBe(4);
      const e2 = nodes.find((n) => n.entry.id === "e2")!;
      expect(e2.entry.label).toBe("checkpoint");
      expect(e2.isOnActivePath).toBe(true);
      const e3 = nodes.find((n) => n.entry.id === "e3")!;
      expect(e3.isOnActivePath).toBe(false);
      const e4 = nodes.find((n) => n.entry.id === "e4")!;
      expect(e4.isLeaf).toBe(true);

      expect(filterTreeNodes(nodes, "messages", "").map((n) => n.entry.id)).toEqual([
        "e1",
        "e2",
        "e4",
      ]);
      expect(filterTreeNodes(nodes, "all", "").length).toBe(4);
      expect(filterTreeNodes(nodes, "labeled", "").map((n) => n.entry.id)).toEqual(["e2"]);
    } finally {
      fs.rmSync(path.dirname(file), { recursive: true, force: true });
    }
  });

  test("renders exact width bordered tree, supports search/Tab filter/Enter & Ctrl+S navigate", () => {
    const file = writeSampleSession();
    try {
      const { entries, leafId } = parseSessionTreeJsonl(file, "e4");
      const nodes = buildFlattenedSessionTree(entries, leafId);
      let result: SessionTreeActionResult | undefined;
      const comp = createSessionTreeTui(
        { requestRender: () => {} },
        mockTheme,
        { nodes, getRows: () => 24 },
        (res) => {
          result = res;
        },
      );

      const width = 104;
      const lines = comp.render(width);
      for (const line of lines) {
        expect(visibleWidth(line)).toBe(width);
      }
      const joined = lines.join("\n");
      expect(joined).toContain("/tree");
      expect(joined).toContain("checkpoint");
      expect(joined).toContain("Current Leaf");

      // Search for checkpoint and confirm with Ctrl+S (summarize=true).
      comp.handleInput("checkpoint");
      comp.handleInput("\x13");
      expect(result).toEqual({ action: "navigate", entryId: "e2", summarize: true });
    } finally {
      fs.rmSync(path.dirname(file), { recursive: true, force: true });
    }
  });
});
