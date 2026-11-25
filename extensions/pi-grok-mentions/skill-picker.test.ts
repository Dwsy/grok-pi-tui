import { describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import {
  buildSkillRecords,
  filterSkillRecords,
  formatDisplayPath,
  loadSkillPreviewDetail,
  parseSkillMarkdown,
  type SkillPreviewDetail,
  type SkillRecord,
} from "./skill-data.ts";
import { createSkillPickerTui } from "./skill-picker-tui.ts";
import type { SessionPickerTheme } from "./session-picker-tui.ts";

const plainTheme: SessionPickerTheme = {
  fg: (_color: string, text: string) => text,
  bold: (text: string) => text,
};

function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;]*m/g, "");
}

function makeSampleSkills(): SkillRecord[] {
  return buildSkillRecords(
    [
      {
        id: "workhub",
        title: "workhub",
        subtitle: "工作文档枢纽，管理 docs/ 下的 Issues 与 PRs",
        sourcePath: "/Users/dev/.agents/skills/workhub/SKILL.md",
      },
      {
        id: "grok-pi-rsi",
        title: "grok-pi-rsi",
        subtitle: "Repo RSI runbooks and feature-gating pitfalls",
        sourcePath: "/Users/dev/pi-grok-build/.pi/skills/grok-pi-rsi/SKILL.md",
      },
      {
        id: "oil-ui",
        title: "oil-ui",
        subtitle: "UI/UX design skill for frontend interfaces",
        sourcePath: "/Users/dev/.agents/skills/oil-ui/SKILL.md",
      },
    ],
    "/Users/dev/pi-grok-build",
  );
}

describe("skill-data parsing, classification, and filtering", () => {
  test("parseSkillMarkdown extracts frontmatter and strips it from body", () => {
    const raw = `---\nname: workhub\ndescription: "SSOT docs manager"\n---\n# Workhub\n\nUse lib.ts to manage PRs.`;
    const parsed = parseSkillMarkdown(raw);
    expect(parsed.frontmatter.name).toBe("workhub");
    expect(parsed.frontmatter.description).toBe("SSOT docs manager");
    expect(parsed.body).toBe("# Workhub\n\nUse lib.ts to manage PRs.");
  });

  test("buildSkillRecords classifies project vs global and puts project skills first", () => {
    const records = makeSampleSkills();
    expect(records).toHaveLength(3);
    expect(records[0]!.id).toBe("grok-pi-rsi");
    expect(records[0]!.origin).toBe("project");
    expect(records[0]!.displayPath).toBe("./.pi/skills/grok-pi-rsi/SKILL.md");
    expect(records[1]!.origin).toBe("global");
    expect(formatDisplayPath("/Users/dev/pi-grok-build/.pi/skills/a/SKILL.md", "/Users/dev/pi-grok-build")).toBe(
      "./.pi/skills/a/SKILL.md",
    );
  });

  test("loadSkillPreviewDetail reads SKILL.md and detects companion folders", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-skill-preview-"));
    try {
      const skillMd = path.join(dir, "SKILL.md");
      fs.writeFileSync(skillMd, "---\nname: demo\ndescription: Demo skill\n---\n# Demo\nStep 1\n", "utf8");
      fs.mkdirSync(path.join(dir, "scripts"));
      fs.writeFileSync(path.join(dir, "scripts", "run.sh"), "#!/bin/sh\n", "utf8");
      fs.mkdirSync(path.join(dir, "references"));
      fs.writeFileSync(path.join(dir, "references", "guide.md"), "# Guide\n", "utf8");

      const detail = loadSkillPreviewDetail(skillMd);
      expect(detail).not.toBeNull();
      expect(detail!.frontmatter.name).toBe("demo");
      expect(detail!.body).toContain("# Demo");
      expect(detail!.lineCount).toBeGreaterThanOrEqual(6);
      expect(detail!.bundledDirs).toEqual(["scripts(1)", "references(1)"]);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("filterSkillRecords supports global/project scope and ranked multi-term search", () => {
    const records = makeSampleSkills();
    expect(filterSkillRecords(records, { scope: "global", query: "" })).toHaveLength(3);
    expect(filterSkillRecords(records, { scope: "project", query: "" }).map((h) => h.record.id)).toEqual([
      "grok-pi-rsi",
    ]);

    const hits = filterSkillRecords(records, { scope: "global", query: "docs pr" });
    expect(hits).toHaveLength(1);
    expect(hits[0]!.record.id).toBe("workhub");
  });
});

describe("SkillPickerTui rendering, keyboard, and mouse interactions", () => {
  const details = new Map<string, SkillPreviewDetail>([
    [
      "/Users/dev/pi-grok-build/.pi/skills/grok-pi-rsi/SKILL.md",
      {
        frontmatter: { name: "grok-pi-rsi", description: "Repo RSI runbooks" },
        body: Array.from({ length: 18 }, (_, i) =>
          i === 0 ? "# Grok Pi RSI" : `- Rule ${i}: feature gating and verification checklist`,
        ).join("\n"),
        lineCount: 42,
        sizeBytes: 2450,
        modified: "2026-11-21 10:30",
        bundledDirs: ["references(2)"],
      },
    ],
    [
      "/Users/dev/.agents/skills/workhub/SKILL.md",
      {
        frontmatter: { name: "workhub", description: "Workhub SSOT" },
        body: "# Workhub\n\nManage Issues and PRs under docs/.",
        lineCount: 257,
        sizeBytes: 7572,
        modified: "2026-11-20 09:00",
        bundledDirs: ["scripts(1)"],
      },
    ],
  ]);

  test("renders full bordered left/right layout with exact width and skill metadata + SKILL.md preview", () => {
    const picker = createSkillPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleSkills(),
        loadDetail: (p) => (p ? details.get(p) ?? null : null),
        getRows: () => 28,
      },
      () => {},
    );

    const width = 104;
    const lines = picker.render(width);
    expect(lines.length).toBeGreaterThanOrEqual(16);
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
    expect(plain).toContain("[Global 3]");
    expect(plain).toContain("[Project 1]");
    expect(plain).toContain("Scope: Project");
    expect(plain).toContain("Path: ./.pi/skills/grok-pi-rsi/SKILL.md");
    expect(plain).toContain("Updated: 2026-11-21 10:30");
    expect(plain).toContain("Size: 42L / 2.4 KB");
    expect(plain).toContain("Extras: references(2)");
    expect(plain).toContain("# Grok Pi RSI");
  });

  test("supports keyboard search, Tab scope toggle, and Enter confirm", () => {
    let chosen: SkillRecord | undefined;
    const picker = createSkillPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleSkills(),
        loadDetail: (p) => (p ? details.get(p) ?? null : null),
        getRows: () => 28,
      },
      (sel) => {
        chosen = sel;
      },
    );

    expect(picker.getScope()).toBe("global");
    picker.render(100);

    picker.handleInput("\t");
    expect(picker.getScope()).toBe("project");
    expect(picker.getSelectedRecord()?.id).toBe("grok-pi-rsi");

    picker.handleInput("\t");
    expect(picker.getScope()).toBe("global");

    picker.handleInput("workhub");
    expect(picker.getSelectedRecord()?.id).toBe("workhub");

    picker.handleInput("\r");
    expect(chosen?.id).toBe("workhub");
  });

  test("supports mouse click on scope pills, left row selection, double-click confirm, and wheel scrolling", () => {
    let chosen: SkillRecord | undefined;
    let closed = false;
    const picker = createSkillPickerTui(
      { requestRender: () => {} },
      plainTheme,
      {
        records: makeSampleSkills(),
        loadDetail: (p) => (p ? details.get(p) ?? null : null),
        getRows: () => 28,
      },
      (sel) => {
        closed = true;
        chosen = sel;
      },
    );

    const lines = picker.render(104);
    const headerPlain = stripAnsi(lines[1]!);
    const projectCol = headerPlain.indexOf("[Project 1]");
    const globalCol = headerPlain.indexOf("[Global 3]");

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

    picker.handleMouse(mouse({ x: projectCol + 2, y: 1 }));
    expect(picker.getScope()).toBe("project");

    picker.handleMouse(mouse({ x: globalCol + 2, y: 1 }));
    expect(picker.getScope()).toBe("global");
    picker.render(104);

    // Click second skill row (`oil-ui`, rows y=7..8).
    picker.handleMouse(mouse({ x: 5, y: 7 }));
    expect(picker.getSelectedRecord()?.id).toBe("oil-ui");

    // Wheel up on left pane moves back to `grok-pi-rsi`.
    picker.handleMouse(mouse({ type: "wheel", button: "none", x: 5, y: 6, wheelDelta: -1 }));
    expect(picker.getSelectedRecord()?.id).toBe("grok-pi-rsi");

    // Wheel down on right pane scrolls SKILL.md preview.
    const beforePreview = picker.render(104).join("\n");
    picker.handleMouse(mouse({ type: "wheel", button: "none", x: 60, y: 10, wheelDelta: 1 }));
    const afterPreview = picker.render(104).join("\n");
    expect(picker.getSelectedRecord()?.id).toBe("grok-pi-rsi");
    expect(afterPreview).not.toBe(beforePreview);

    // Double-click row 2 (`workhub`, y=9..10) confirms selection.
    picker.handleMouse(mouse({ type: "click", button: "left", x: 5, y: 9, clickCount: 2 }));
    expect(closed).toBe(true);
    expect(chosen?.id).toBe("workhub");
  });

  test("coalesces rapid preview-wheel events without reloading detail", async () => {
    let renderRequests = 0;
    let detailCalls = 0;
    const picker = createSkillPickerTui(
      {
        requestRender: () => {
          renderRequests += 1;
        },
      },
      plainTheme,
      {
        records: makeSampleSkills(),
        initialScope: "project",
        loadDetail: (p) => {
          detailCalls += 1;
          return p ? details.get(p) ?? null : null;
        },
        getRows: () => 28,
      },
      () => {},
    );

    picker.render(104);
    expect(renderRequests).toBe(0);
    expect(detailCalls).toBe(1);

    const mouse = (overrides: Partial<TuiMouseEvent>): TuiMouseEvent => ({
      type: "wheel",
      button: "none",
      x: 60,
      y: 10,
      screenX: 60,
      screenY: 10,
      width: 104,
      height: 25,
      shift: false,
      alt: false,
      ctrl: false,
      ...overrides,
    });
    const results = [];
    for (let i = 0; i < 8; i++) {
      results.push(picker.handleMouse(mouse({ wheelDelta: 1 })));
    }
    expect(results).toHaveLength(8);
    expect(results.every((result) => result?.handled === true && result.render === false)).toBe(true);
    expect(renderRequests).toBe(0);
    expect(detailCalls).toBe(1);

    await new Promise((resolve) => setTimeout(resolve, 70));
    expect(renderRequests).toBe(1);
    expect(detailCalls).toBe(1);
  });
});
