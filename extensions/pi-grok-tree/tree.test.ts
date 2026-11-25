import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, expect, test } from "bun:test";
import { visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
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

function countedSplitText(value: string, onSplit: () => void): string {
  const text = new String(value) as unknown as {
    split: (separator: RegExp | string, limit?: number) => string[];
  };
  const originalSplit = String.prototype.split;
  text.split = (separator, limit) => {
    onSplit();
    return originalSplit.call(value, separator, limit);
  };
  return text as unknown as string;
}

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

      expect(filterTreeNodes(nodes, "no-tools", "").map((n) => n.entry.id)).toEqual([
        "e1",
        "e2",
        "e4",
      ]);
      expect(filterTreeNodes(nodes, "user-only", "").map((n) => n.entry.id)).toEqual([
        "e1",
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

  test("linear chains stay flat without diagonal staircase indentation, and supports 90% height, mouse, and pi-main shortcuts", () => {
    const file = writeSampleSession();
    try {
      const { entries, leafId } = parseSessionTreeJsonl(file, "e4");
      const nodes = buildFlattenedSessionTree(entries, leafId);
      // e1 -> e2 is a single linear chain: both stay at depth 0 with empty prefix.
      const e1 = nodes.find((n) => n.entry.id === "e1")!;
      const e2 = nodes.find((n) => n.entry.id === "e2")!;
      // e2 branches into e3 and e4: they get ├─ / └─ connectors at depth 0 (children recurse at depth 1).
      const e3 = nodes.find((n) => n.entry.id === "e3")!;
      const e4 = nodes.find((n) => n.entry.id === "e4")!;
      expect(e1.depth).toBe(0);
      expect(e1.prefix).toBe("");
      expect(e2.depth).toBe(0);
      expect(e2.prefix).toBe("");
      expect(e2.isBranchPoint).toBe(true);
      expect(e3.prefix).toBe("├─ ");
      expect(e4.prefix).toBe("└─ ");

      let copied = "";
      let result: SessionTreeActionResult | undefined;
      const comp = createSessionTreeTui(
        { requestRender: () => {} },
        mockTheme,
        {
          nodes,
          getRows: () => 50,
          onCopy: (text) => {
            copied = text;
          },
        },
        (res) => {
          result = res;
        },
      ) as ReturnType<typeof createSessionTreeTui> & {
        handleMouse: (ev: Record<string, unknown>) => unknown;
      };

      // 90% of 50 rows = 45 rows (not capped at 34).
      const lines = comp.render(110);
      expect(lines.length).toBe(45);

      // Shift+T toggles timestamps on rows.
      comp.handleInput("T");
      expect(comp.render(110).join("\n")).toContain("11-23");

      // Ctrl+X copies selected node text.
      comp.handleInput("\x18");
      expect(copied).toContain("Alternate branch question");

      // Mouse click on first row (y=5) selects e1, double-click jumps to e1.
      comp.handleMouse({
        type: "press",
        button: "left",
        x: 5,
        y: 5,
        screenX: 5,
        screenY: 5,
        width: 110,
        height: 45,
        shift: false,
        alt: false,
        ctrl: false,
      });
      comp.handleMouse({
        type: "click",
        button: "left",
        clickCount: 2,
        x: 5,
        y: 5,
        screenX: 5,
        screenY: 5,
        width: 110,
        height: 45,
        shift: false,
        alt: false,
        ctrl: false,
      });
      expect(result).toEqual({ action: "navigate", entryId: "e1", summarize: false });
    } finally {
      fs.rmSync(path.dirname(file), { recursive: true, force: true });
    }
  });

  test("coalesces right-pane wheel renders and caches preview wrapping", async () => {
    const file = writeSampleSession();
    try {
      const { entries, leafId } = parseSessionTreeJsonl(file, "e4");
      const nodes = buildFlattenedSessionTree(entries, leafId);
      const leaf = nodes.find((n) => n.isLeaf)!;
      let wrapPasses = 0;
      leaf.entry.fullText = countedSplitText(
        Array.from({ length: 40 }, (_, i) => `preview line ${i} ${`x`.repeat(70)}`).join("\n"),
        () => {
          wrapPasses += 1;
        },
      );
      let renderRequests = 0;
      const comp = createSessionTreeTui(
        {
          requestRender: () => {
            renderRequests += 1;
          },
        },
        mockTheme,
        { nodes, getRows: () => 50 },
        () => {},
      ) as ReturnType<typeof createSessionTreeTui> & {
        handleMouse: (ev: TuiMouseEvent) => unknown;
      };

      comp.render(110);
      expect(wrapPasses).toBe(1);

      const wheel = (): TuiMouseEvent => ({
        type: "wheel",
        button: "none",
        wheelDelta: 1,
        x: 80,
        y: 5,
        screenX: 80,
        screenY: 5,
        width: 110,
        height: 45,
        shift: false,
        alt: false,
        ctrl: false,
      });
      for (let i = 0; i < 8; i += 1) comp.handleMouse(wheel());
      expect(renderRequests).toBe(0);

      await new Promise((resolve) => setTimeout(resolve, 70));
      expect(renderRequests).toBeGreaterThanOrEqual(1);

      const renderRequestsAfterCoalesce = renderRequests;
      for (let i = 0; i < 3; i += 1) comp.render(110);
      expect(wrapPasses).toBe(1);
      expect(renderRequests).toBe(renderRequestsAfterCoalesce);
    } finally {
      fs.rmSync(path.dirname(file), { recursive: true, force: true });
    }
  });
});
