import { describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import {
  buildVisibleTreeNodes,
  detectNerdFontSupport,
  filterFileEntries,
  getFileIcon,
  grepWorkspaceWithRg,
  listWorkspaceFilesWithFd,
  loadFileOrDirPreview,
  parseRgVimgrepOutput,
} from "./file-data.ts";
import { createFilePickerTui, type FilePickerSelection } from "./file-picker-tui.ts";
import type { SessionPickerTheme } from "./session-picker-tui.ts";

const plainTheme: SessionPickerTheme = {
  fg: (_color: string, text: string) => text,
  bold: (text: string) => text,
};

function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;]*m/g, "");
}

function makeTempWorkspace(): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-file-picker-"));
  fs.mkdirSync(path.join(dir, "src", "views"), { recursive: true });
  fs.mkdirSync(path.join(dir, ".hidden-dir"), { recursive: true });
  fs.writeFileSync(
    path.join(dir, "Cargo.toml"),
    '[package]\nname = "demo"\nversion = "0.1.0"\n',
    "utf8",
  );
  fs.writeFileSync(
    path.join(dir, "src", "main.rs"),
    [
      "fn main() {",
      '    println!("hello grok-pi");',
      "    let needle_token = 42;",
      "}",
    ].join("\n"),
    "utf8",
  );
  fs.writeFileSync(
    path.join(dir, "src", "views", "picker.ts"),
    Array.from({ length: 25 }, (_, i) =>
      i === 14 ? "export const TARGET_NEEDLE = 'found-by-rg';" : `// line ${i + 1} preview content`,
    ).join("\n"),
    "utf8",
  );
  fs.writeFileSync(path.join(dir, ".env"), "SECRET=1\n", "utf8");
  fs.writeFileSync(path.join(dir, ".hidden-dir", "note.md"), "# Hidden Note\n", "utf8");
  // Small binary fixture with NUL byte
  fs.writeFileSync(path.join(dir, "logo.png"), Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x00, 0x01]));
  return dir;
}

describe("file-data Nerd icons, lazy tree, fd search, rg grep, and preview", () => {
  test("detects Nerd Font support and provides devicon + ASCII fallback glyphs", () => {
    expect(detectNerdFontSupport({ PI_GROK_NERD_ICONS: "1" })).toBe(true);
    expect(detectNerdFontSupport({ PI_GROK_NERD_ICONS: "0" })).toBe(false);
    expect(detectNerdFontSupport({ TERM_PROGRAM: "ghostty" })).toBe(true);

    const rsNerd = getFileIcon("main.rs", { nerdIcons: true });
    const rsAscii = getFileIcon("main.rs", { nerdIcons: false });
    expect(rsNerd.glyph).toBe("");
    expect(rsAscii.glyph).toBe("·");

    const dirClosed = getFileIcon("src", { isDir: true, expanded: false, nerdIcons: true });
    const dirOpen = getFileIcon("src", { isDir: true, expanded: true, nerdIcons: true });
    expect(dirClosed.glyph).toBe("");
    expect(dirOpen.glyph).toBe("");
  });

  test("buildVisibleTreeNodes lazily expands only opened directories and respects hidden toggle", () => {
    const dir = makeTempWorkspace();
    try {
      const collapsed = buildVisibleTreeNodes(dir, new Set(), { showHidden: false });
      expect(collapsed.map((n) => n.relPath)).toEqual(["src", "Cargo.toml", "logo.png"]);

      const expanded = buildVisibleTreeNodes(dir, new Set(["src"]), { showHidden: false });
      expect(expanded.map((n) => n.relPath)).toEqual([
        "src",
        "src/views",
        "src/main.rs",
        "Cargo.toml",
        "logo.png",
      ]);

      const withHidden = buildVisibleTreeNodes(dir, new Set(), { showHidden: true });
      expect(withHidden.some((n) => n.name === ".env")).toBe(true);
      expect(withHidden.some((n) => n.name === ".hidden-dir")).toBe(true);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("listWorkspaceFilesWithFd + filterFileEntries ranks smart fuzzy matches", () => {
    const dir = makeTempWorkspace();
    try {
      const all = listWorkspaceFilesWithFd(dir, { showHidden: false });
      expect(all.some((e) => e.relPath === "src/views/picker.ts")).toBe(true);
      expect(all.some((e) => e.relPath === ".env")).toBe(false);

      const hidden = listWorkspaceFilesWithFd(dir, { showHidden: true });
      expect(hidden.some((e) => e.relPath === ".env")).toBe(true);

      const ranked = filterFileEntries(all, "pkr");
      expect(ranked[0]?.relPath).toBe("src/views/picker.ts");
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("grepWorkspaceWithRg finds full-text matches and handles partial regex safely", () => {
    const dir = makeTempWorkspace();
    try {
      const hits = grepWorkspaceWithRg(dir, "TARGET_NEEDLE", { showHidden: false });
      expect(hits).toHaveLength(1);
      expect(hits[0]!.relPath).toBe("src/views/picker.ts");
      expect(hits[0]!.line).toBe(15);

      // Incomplete regex `println!(` falls back to --fixed-strings without failing.
      const literalHits = grepWorkspaceWithRg(dir, "println!(", { showHidden: false });
      expect(literalHits).toHaveLength(1);
      expect(literalHits[0]!.relPath).toBe("src/main.rs");

      const parsed = parseRgVimgrepOutput("./src/main.rs:2:5:    println!(\"hi\");\n");
      expect(parsed[0]).toEqual({
        relPath: "src/main.rs",
        line: 2,
        column: 5,
        text: '    println!("hi");',
      });
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("loadFileOrDirPreview handles text files, directories, and binary files cleanly", () => {
    const dir = makeTempWorkspace();
    try {
      const filePrev = loadFileOrDirPreview(dir, "src/main.rs", { showHidden: false, nerdIcons: true });
      expect(filePrev.kind).toBe("file");
      expect(filePrev.lineCount).toBe(4);

      const dirPrev = loadFileOrDirPreview(dir, "src", { showHidden: false, nerdIcons: true });
      expect(dirPrev.kind).toBe("dir");
      expect(dirPrev.dirSummary).toBe("1 dirs · 1 files");

      const binPrev = loadFileOrDirPreview(dir, "logo.png", { showHidden: false, nerdIcons: true });
      expect(binPrev.kind).toBe("binary");
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("FilePickerTui rendering, modes (Tree / fd / rg), keyboard, and mouse", () => {
  test("renders bordered Tree + Preview layout with exact display width and Nerd/ASCII toggle", () => {
    const dir = makeTempWorkspace();
    try {
      const picker = createFilePickerTui(
        { requestRender: () => {} },
        plainTheme,
        { rootDir: dir, nerdIcons: true, getRows: () => 28 },
        () => {},
      );

      const width = 108;
      const lines = picker.render(width);
      expect(lines.length).toBeGreaterThanOrEqual(16);
      for (const line of lines) {
        expect(visibleWidth(line)).toBe(width);
      }
      const plain = lines.map(stripAnsi).join("\n");
      expect(lines[0]!.startsWith("╭")).toBe(true);
      expect(lines[0]!.endsWith("╮")).toBe(true);
      expect(plain).toContain("[Tree]");
      expect(plain).toContain("[Files·fd]");
      expect(plain).toContain("[Grep·rg]");
      expect(plain).toContain("[Nerd:on]");
      expect(plain).toContain("src/");

      // Expand `src` with Right arrow, verify indent guides appear.
      picker.handleInput("\x1b[C");
      const expandedPlain = picker.render(width).map(stripAnsi).join("\n");
      expect(expandedPlain).toContain("main.rs");
      expect(expandedPlain).toContain("├─");
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("switches Tree -> fd -> rg via Tab and inserts @path:line from rg grep hit", () => {
    const dir = makeTempWorkspace();
    try {
      let chosen: FilePickerSelection | undefined;
      const picker = createFilePickerTui(
        { requestRender: () => {} },
        plainTheme,
        { rootDir: dir, nerdIcons: false, getRows: () => 28 },
        (sel) => {
          chosen = sel;
        },
      );

      expect(picker.getMode()).toBe("tree");
      picker.handleInput("\t");
      expect(picker.getMode()).toBe("files");
      picker.handleInput("\t");
      expect(picker.getMode()).toBe("grep");

      picker.handleInput("TARGET_NEEDLE");
      const rendered = picker.render(108).map(stripAnsi).join("\n");
      expect(rendered).toContain("src/views/picker.ts:15");
      expect(rendered).toContain("Line 15");
      expect(rendered).toContain("▸");

      picker.handleInput("\r");
      expect(chosen?.insertText).toBe("@src/views/picker.ts:15");
      expect(chosen?.line).toBe(15);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("supports mouse click on mode pills, Hidden/Nerd toggles, row select, and double-click insert", () => {
    const dir = makeTempWorkspace();
    try {
      let chosen: FilePickerSelection | undefined;
      const picker = createFilePickerTui(
        { requestRender: () => {} },
        plainTheme,
        { rootDir: dir, nerdIcons: false, getRows: () => 28 },
        (sel) => {
          chosen = sel;
        },
      );

      const lines = picker.render(108);
      const header = stripAnsi(lines[1]!);
      const filesCol = header.indexOf("[Files·fd]");
      const hiddenCol = header.indexOf("[Hidden]");
      const nerdCol = header.indexOf("[Nerd]");

      const mouse = (overrides: Partial<TuiMouseEvent>): TuiMouseEvent => ({
        type: "press",
        button: "left",
        x: 0,
        y: 0,
        screenX: 0,
        screenY: 0,
        width: 108,
        height: lines.length,
        shift: false,
        alt: false,
        ctrl: false,
        ...overrides,
      });

      // Click [Files·fd] pill.
      picker.handleMouse(mouse({ x: filesCol + 2, y: 1 }));
      expect(picker.getMode()).toBe("files");

      // Click [Hidden] and [Nerd] toggles.
      picker.handleMouse(mouse({ x: hiddenCol + 2, y: 1 }));
      expect(picker.isShowingHidden()).toBe(true);
      picker.handleMouse(mouse({ x: nerdCol + 2, y: 1 }));
      expect(picker.isUsingNerdIcons()).toBe(true);

      // Re-render and double-click first row (y=5) to confirm selection.
      picker.render(108);
      picker.handleMouse(mouse({ type: "click", button: "left", x: 5, y: 5, clickCount: 2 }));
      expect(chosen).toBeDefined();
      expect(chosen!.insertText.startsWith("@")).toBe(true);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
