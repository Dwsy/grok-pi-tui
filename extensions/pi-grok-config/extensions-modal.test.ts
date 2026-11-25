import { describe, expect, test } from "bun:test";
import { visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { createExtensionsModalTui, mapRustTabName } from "./extensions-modal-tui.ts";
import type { PiConfigMutationPayload, PiConfigSnapshot } from "./config-data.ts";

const plainTheme = {
  fg: (_color: string, text: string) => text,
  bold: (text: string) => text,
};

function stripAnsi(text: string): string {
  return text.replace(/\x1b\[[0-9;]*m/g, "");
}

function makeSnapshot(): PiConfigSnapshot {
  return {
    cwd: "/tmp/project",
    agentDir: "/tmp/agent",
    projectTrusted: true,
    resources: [
      {
        id: "ext-1",
        displayName: "my-plugin",
        path: "/tmp/agent/extensions/my-plugin/index.ts",
        resourceType: "Extensions",
        scope: "Global",
        origin: "auto",
        source: "local",
        enabled: true,
        inheritedEnabled: true,
        projectOverride: "Inherit",
      },
      {
        id: "skill-1",
        displayName: "deploy-helper",
        path: "/tmp/agent/skills/deploy-helper/SKILL.md",
        resourceType: "Skills",
        scope: "Global",
        origin: "auto",
        source: "local",
        enabled: false,
        inheritedEnabled: false,
        projectOverride: "Inherit",
      },
    ],
  };
}

describe("ExtensionsModalTui", () => {
  test("maps Rust tab names, filters by tab/status, mutates resources, and drags divider", () => {
    expect(mapRustTabName("McpServers")).toBe("mcp");
    expect(mapRustTabName("Skills")).toBe("skills");
    expect(mapRustTabName("Workflows")).toBe("workflows");

    const mutations: PiConfigMutationPayload[] = [];
    let closed = false;
    const tui = createExtensionsModalTui(
      { requestRender: () => {} },
      plainTheme,
      {
        snapshot: makeSnapshot(),
        mcpServers: [
          {
            id: "github",
            name: "github",
            endpoint: "pi-mcp",
            transport: "mcp",
            tools: [{ name: "list_prs", description: "List pull requests" }],
          },
        ],
        initialTab: "plugins",
        onMutate: (p) => mutations.push(p),
        getRows: () => 26,
      },
      () => {
        closed = true;
      },
    );

    const width = 104;
    const lines = tui.render(width);
    for (const line of lines) expect(visibleWidth(line)).toBe(width);
    expect(lines.map(stripAnsi).join("\n")).toContain("my-plugin");

    // Space toggles enabled -> disabled and emits pi_config_mutate payload.
    tui.handleInput(" ");
    expect(mutations).toHaveLength(1);
    expect(mutations[0]?.enabled).toBe(false);

    // Tab switches Plugins -> Skills -> MCP.
    tui.handleInput("\t");
    expect(tui.getActiveTab()).toBe("skills");
    expect(tui.getSelectedEntry()?.name).toBe("deploy-helper");

    tui.handleInput("\t");
    expect(tui.getActiveTab()).toBe("mcp");
    expect(tui.render(width).map(stripAnsi).join("\n")).toContain("list_prs");

    // Divider drag updates split geometry cleanly.
    const dividerX = lines[6]!.indexOf("│", 10);
    const makeMouse = (type: TuiMouseEvent["type"], x: number): TuiMouseEvent => ({
      type,
      button: type === "release" ? "none" : "left",
      x,
      y: 8,
      screenX: x,
      screenY: 8,
      width,
      height: lines.length,
      shift: false,
      alt: false,
      ctrl: false,
    });
    expect(tui.handleMouse(makeMouse("press", dividerX))?.capture).toBe(true);
    tui.handleMouse(makeMouse("drag", 60));
    tui.handleMouse(makeMouse("release", 60));
    const resized = tui.render(width);
    expect(resized[6]!.indexOf("│", 10)).toBeGreaterThan(dividerX);

    tui.handleInput("\x1b");
    expect(closed).toBe(true);
  });
});
