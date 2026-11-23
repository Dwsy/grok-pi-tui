import { describe, expect, test } from "bun:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import {
  createMcpPickerTui,
  filterMcpServers,
  type McpServerRecord,
} from "./mcp-picker-tui.ts";
import type { TuiTheme } from "../pi-grok-tui-kit/index.ts";

const mockTheme: TuiTheme = {
  fg: (_color: string, text: string) => text,
  bg: (_color: string, text: string) => text,
  bold: (text: string) => text,
};

const sampleServers: McpServerRecord[] = [
  {
    id: "nexusdock",
    name: "nexusdock",
    endpoint: "https://cloud.nexusdock.dev/mcp",
    transport: "http",
    tools: [
      {
        name: "exec_command",
        fullName: "mcp__nexusdock__exec_command",
        description: "Run a bounded command on a connected node",
      },
      {
        name: "read_file",
        fullName: "mcp__nexusdock__read_file",
        description: "Read a UTF-8 file slice from the host",
      },
    ],
    searchText: "nexusdock https://cloud.nexusdock.dev/mcp exec_command read_file bounded command",
  },
  {
    id: "sqlite-local",
    name: "sqlite-local",
    endpoint: "npx -y @modelcontextprotocol/server-sqlite",
    transport: "stdio",
    tools: [
      {
        name: "query",
        fullName: "mcp__sqlite-local__query",
        description: "Execute read-only SQL queries against SQLite",
      },
    ],
    searchText: "sqlite-local npx server-sqlite query SQL",
  },
  {
    id: "empty-server",
    name: "empty-server",
    endpoint: "http://127.0.0.1:9999/mcp",
    transport: "http",
    tools: [],
    searchText: "empty-server http://127.0.0.1:9999/mcp",
  },
];

describe("mcp-picker-tui filtering, rendering, keyboard, and mouse", () => {
  test("filterMcpServers supports All / With Tools filter and multi-term search", () => {
    expect(filterMcpServers(sampleServers, "", "all").length).toBe(3);
    expect(filterMcpServers(sampleServers, "", "with-tools").length).toBe(2);

    const byTool = filterMcpServers(sampleServers, "exec_command", "all");
    expect(byTool.map((r) => r.id)).toEqual(["nexusdock"]);

    const byDesc = filterMcpServers(sampleServers, "sqlite sql", "all");
    expect(byDesc.map((r) => r.id)).toEqual(["sqlite-local"]);
  });

  test("renders bordered split frame with exact display width and server/tool preview", () => {
    const comp = createMcpPickerTui(
      { requestRender: () => {} },
      mockTheme,
      { records: sampleServers, getRows: () => 24 },
      () => {},
    );
    const width = 104;
    const lines = comp.render(width);
    expect(lines.length).toBeGreaterThanOrEqual(12);
    for (const line of lines) {
      expect(visibleWidth(line)).toBe(width);
    }
    const joined = lines.join("\n");
    expect(joined).toContain("@mcp");
    expect(joined).toContain("nexusdock");
    expect(joined).toContain("exec_command");
    expect(joined).toContain("read_file");
  });

  test("supports keyboard search, Tab filter toggle, navigation, and Enter confirm", () => {
    let chosen: McpServerRecord | undefined;
    const comp = createMcpPickerTui(
      { requestRender: () => {} },
      mockTheme,
      { records: sampleServers, getRows: () => 24 },
      (res) => {
        chosen = res;
      },
    );
    comp.render(100);
    comp.handleInput("\t"); // All -> With Tools
    expect(comp.render(100).join("\n")).toContain("(2/3)");

    comp.handleInput("sqlite");
    comp.handleInput("\r");
    expect(chosen?.id).toBe("sqlite-local");
  });

  test("supports mouse click select and double-click confirm", () => {
    let chosen: McpServerRecord | undefined;
    const comp = createMcpPickerTui(
      { requestRender: () => {} },
      mockTheme,
      { records: sampleServers, getRows: () => 24 },
      (res) => {
        chosen = res;
      },
    );
    comp.render(100);
    // Body starts at 0-based row 5 -> 1-based row 6. Second server is at 0-based row 7 -> 1-based row 8.
    comp.handleInput("\x1b[<0;6;8M");
    comp.handleInput("\x1b[<0;6;8m");
    expect(chosen).toBeUndefined();
    comp.handleInput("\x1b[<0;6;8M");
    expect(chosen?.id).toBe("sqlite-local");
  });
});
