import { expect, test } from "bun:test";
import { visibleWidth } from "../../pi-main/packages/tui/src/utils.ts";
import { formatTimelineBatch, TIMELINE_TOOL_GROUPS, type FormatRequest } from "./timeline-format.ts";

function format(entries: FormatRequest["entries"]) {
  return formatTimelineBatch({
    op: "format", entries, id: 42, tickColumns: 2, previewColumns: 32,
    scope: { host: "host", session: "session", branch: 2, theme: 3 },
  });
}

const unstyle = (text: string) => text.replace(/\x1b\[[0-9;]*m/g, "");

test("prompt hover shows the input and the beginning of the final reply within its column budget", () => {
  const result = format([{
    markerId: 4,
    version: "reply",
    entry: {
      kind: "prompt", prompt: "# Original question",
      reply: "## Final reply\n**已经完成**. The opening stays visible. " + "Trailing text ".repeat(20),
    },
  }]).entries[0]!;
  expect(result.markerId).toBe(4);
  expect(result.version).toBe("reply");
  expect(result.previewAnsi?.map(unstyle)[0]).toBe("Original question");
  expect(result.previewAnsi?.map(unstyle)[1]).toBe("─".repeat(32));
  expect(result.previewAnsi?.map(unstyle).slice(2).join(" ")).toContain("Final reply 已经完成");
  expect(result.previewAnsi).toHaveLength(5);
  expect(result.previewAnsi?.every((line) => visibleWidth(line) <= 32)).toBe(true);
});

test("an unanswered prompt shows only its input, while compaction shows the summary", () => {
  const result = format([
    { markerId: 1, version: "prompt", entry: { kind: "prompt", prompt: "Workspace status?" } },
    { markerId: 2, version: "summary", entry: { kind: "compaction", summary: "**Earlier context** is preserved." } },
  ]).entries;
  expect(result[0]?.previewAnsi).toEqual(["Workspace status?"]);
  expect(result[1]?.tickAnsi && unstyle(result[1].tickAnsi)).toBe("◆");
  expect(result[1]?.previewAnsi?.map(unstyle)).toEqual([
    "Compaction summary",
    "─".repeat(32),
    "Earlier context is preserved.",
  ]);
});

test("each built-in interaction name has a distinct family tick and bounded status preview", () => {
  for (const [index, group] of TIMELINE_TOOL_GROUPS.entries()) {
    const tools = group.tools.map((toolName, index) => ({
      markerId: index + 1, version: "tool",
      entry: {
        kind: "tool" as const, callId: String(index), toolName, status: "failed" as const,
        interaction: { target: "/root/reviewer", mode: "follow_up" },
      },
    }));
    for (const formatted of format(tools).entries) {
      expect(formatted.tickAnsi && unstyle(formatted.tickAnsi)).toBe(index === 0 ? "S" : "T");
      expect(formatted.previewAnsi?.map(unstyle).slice(1)).toEqual(["/root/reviewer · follow up", "failed"]);
      expect(formatted.previewAnsi?.every((line) => visibleWidth(line) <= 32)).toBe(true);
    }
  }
});

test("prompt preview wraps user prompt to 2 lines, adds dividers, and colors +green/-red file diffs", () => {
  const items = format([{
    markerId: 6,
    version: "changes",
    entry: {
      kind: "prompt", prompt: "Update checkout flow and verify every changed file in detail",
      reply: "Implemented the requested changes and ran the focused checks. ".repeat(5),
      changes: {
        fileCount: 3, added: 4, deleted: 2, written: 24,
        files: [
          { path: "src/nested/path/long/checkout.rs", added: 3, deleted: 2, written: 15 },
          { path: "src/ui.rs", added: 1, deleted: 0, written: 0 },
          { path: "src/extra.rs", added: 0, deleted: 0, written: 9 },
        ],
      },
    },
  }]).entries[0]!;
  const rawRows = items.previewAnsi ?? [];
  const rows = rawRows.map(unstyle);
  expect(items.tickAnsi).toBeUndefined();
  expect(rows[0]).toContain("Update checkout");
  expect(rows[1]).toContain("every changed file");
  expect(rows[2]).toBe("─".repeat(32));
  expect(rows[3]).toContain("Implemented");
  const fileHeaderIdx = rows.findIndex((r) => r.startsWith("3 files"));
  expect(fileHeaderIdx).toBeGreaterThan(3);
  expect(rows[fileHeaderIdx - 1]).toBe("─".repeat(32));
  expect(rows[fileHeaderIdx]).toBe("3 files · +4 -2 · 24 written");
  expect(rawRows[fileHeaderIdx]).toContain("\x1b[32m+4\x1b[0m");
  expect(rawRows[fileHeaderIdx]).toContain("\x1b[31m-2\x1b[0m");
  expect(rows.slice(fileHeaderIdx + 1).join(" ")).toContain("checkout.rs");
  expect(rows.slice(fileHeaderIdx + 1).join(" ")).toContain("src/ui.rs");
  expect(rows.slice(fileHeaderIdx + 1).join(" ")).toContain("src/extra.rs");
  expect(rows.every((row) => visibleWidth(row) <= 32)).toBe(true);
});
