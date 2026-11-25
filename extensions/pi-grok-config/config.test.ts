import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, expect, test } from "bun:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import type { TuiTheme } from "../pi-grok-tui-kit/index.ts";
import {
  filterPiConfigResources,
  loadPiConfigSnapshot,
  type PiConfigMutationPayload,
  type PiConfigSnapshot,
} from "./config-data.ts";
import { createPiConfigTui } from "./config-tui.ts";

const mockTheme: TuiTheme = {
  fg: (_c: string, t: string) => t,
  bg: (_c: string, t: string) => t,
  bold: (t: string) => t,
};

function makeSampleSnapshot(dir: string): PiConfigSnapshot {
  const skillFile = path.join(dir, "SKILL.md");
  const extFile = path.join(dir, "helper.ts");
  fs.writeFileSync(skillFile, "# My Skill\n\nInstructions here.\n", "utf8");
  fs.writeFileSync(extFile, "export default function() {}\n", "utf8");
  return {
    cwd: dir,
    agentDir: path.join(dir, ".pi"),
    projectTrusted: true,
    resources: [
      {
        id: `Skills:${skillFile}`,
        displayName: "my-skill",
        path: skillFile,
        resourceType: "Skills",
        scope: "Global",
        origin: "auto",
        source: "global",
        enabled: true,
        inheritedEnabled: true,
        projectOverride: "Inherit",
      },
      {
        id: `Extensions:${extFile}`,
        displayName: "helper.ts",
        path: extFile,
        resourceType: "Extensions",
        scope: "Project",
        origin: "settings",
        source: "project",
        enabled: true,
        inheritedEnabled: true,
        projectOverride: "Inherit",
      },
    ],
  };
}

describe("pi-grok-config snapshot parsing and TUI renderer", () => {
  test("loads Rust JSON snapshot and filters by type, scope, and search query", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-grok-cfg-test-"));
    try {
      const snap = makeSampleSnapshot(dir);
      const snapPath = path.join(dir, "snap.json");
      fs.writeFileSync(snapPath, JSON.stringify(snap), "utf8");

      const loaded = loadPiConfigSnapshot(snapPath);
      expect(loaded?.resources.length).toBe(2);
      expect(filterPiConfigResources(loaded!.resources, "All", "Global", "").length).toBe(1);
      expect(filterPiConfigResources(loaded!.resources, "All", "Project", "").length).toBe(2);
      expect(filterPiConfigResources(loaded!.resources, "Skills", "Global", "my").length).toBe(1);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("renders exact-width bordered UI and emits global toggle & project override mutations", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-grok-cfg-ui-"));
    try {
      const snap = makeSampleSnapshot(dir);
      const mutations: PiConfigMutationPayload[] = [];
      let closed = false;
      const comp = createPiConfigTui(
        { requestRender: () => {} },
        mockTheme,
        {
          snapshot: snap,
          nerdIcons: false,
          onMutate: (m) => mutations.push(m),
          getRows: () => 24,
        },
        () => {
          closed = true;
        },
      );

      const width = 108;
      const lines = comp.render(width);
      for (const line of lines) {
        expect(visibleWidth(line)).toBe(width);
      }
      const joined = lines.join("\n");
      expect(joined).toContain("/pi-config");
      expect(joined).toContain("my-skill");
      expect(joined).toContain("Instructions here.");

      // Space toggles Global enabled -> false
      comp.handleInput(" ");
      expect(mutations.length).toBe(1);
      expect(mutations[0]?.mode).toBe("global");
      expect(mutations[0]?.enabled).toBe(false);

      // Tab switches to Project scope (matching Rust pi_config.rs), Enter cycles Inherit -> Load
      comp.handleInput("\t");
      comp.handleInput("\r");
      expect(mutations.length).toBe(2);
      expect(mutations[1]?.mode).toBe("project");
      expect(mutations[1]?.projectOverride).toBe("Load");

      comp.handleInput("\x1b");
      expect(closed).toBe(true);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("coalesces rapid right-pane wheel scrolling and cancels on Esc", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-grok-cfg-wheel-"));
    try {
      const snap = makeSampleSnapshot(dir);
      let renderRequests = 0;
      const comp = createPiConfigTui(
        { requestRender: () => { renderRequests += 1; } },
        mockTheme,
        { snapshot: snap, nerdIcons: false, onMutate: () => {}, getRows: () => 24 },
        () => {},
      );
      comp.render(108);
      const before = renderRequests;
      for (let i = 0; i < 6; i++) {
        comp.handleInput("\x1b[<65;85;10M");
      }
      expect(renderRequests).toBe(before);
      await new Promise((r) => setTimeout(r, 70));
      expect(renderRequests).toBe(before + 1);
      comp.handleInput("\x1b");
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
