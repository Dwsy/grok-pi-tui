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

      const imgPrev = loadFileOrDirPreview(dir, "logo.png", { showHidden: false, nerdIcons: true });
      expect(imgPrev.kind).toBe("image");

      fs.writeFileSync(path.join(dir, "blob.bin"), Buffer.from([0x00, 0x01, 0x02, 0x03]));
      const binPrev = loadFileOrDirPreview(dir, "blob.bin", { showHidden: false, nerdIcons: true });
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
        {
          rootDir: dir,
          nerdIcons: false,
          highlightFile: () => undefined,
          getRows: () => 28,
        },
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
        {
          rootDir: dir,
          nerdIcons: false,
          highlightFile: () => undefined,
          getRows: () => 28,
        },
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

  test("emits HD image overlay under Herdr and clears when switching to a text file", () => {
    const dir = makeTempWorkspace();
    try {
      // Minimal valid 2x2 PNG header (IHDR: width=2, height=2)
      const pngBytes = Buffer.from(
        "89504e470d0a1a0a0000000d4948445200000002000000020802000000fdd49a730000000049454e44ae426082",
        "hex",
      );
      fs.writeFileSync(path.join(dir, "00-preview.png"), pngBytes);
      fs.writeFileSync(path.join(dir, "01-notes.txt"), "hello world\n");

      const overlayCalls: Array<unknown> = [];
      const picker = createFilePickerTui(
        { requestRender: () => {} },
        plainTheme,
        {
          rootDir: dir,
          initialMode: "files",
          nerdIcons: false,
          env: { HERDR_ENV: "1", TERM_PROGRAM: "herdr" },
          onImageOverlay: (spec) => overlayCalls.push(spec),
          getRows: () => 28,
        },
        () => {},
      );

      picker.handleInput("00-preview");
      const lines = picker.render(108);
      for (const line of lines) {
        expect(visibleWidth(line)).toBe(108);
      }
      const rendered = lines.map(stripAnsi).join("\n");
      expect(rendered).toContain("PNG 2×2");
      expect(rendered).toContain("HD · Herdr/Kitty");
      expect(overlayCalls.length).toBe(1);
      const spec = overlayCalls[0] as { path: string; x: number; y: number; width: number; height: number };
      expect(spec.path.endsWith("00-preview.png")).toBe(true);
      expect(spec.width).toBeGreaterThanOrEqual(20);
      expect(spec.height).toBeGreaterThanOrEqual(8);

      // Re-rendering the same selection must be diff-checked (zero redundant emissions).
      picker.render(108);
      expect(overlayCalls.length).toBe(1);

      // Switching to a text file clears the overlay.
      picker.handleInput("\x17"); // Ctrl+W clear query
      picker.handleInput("01-notes");
      picker.render(108);
      expect(overlayCalls[overlayCalls.length - 1]).toBeUndefined();
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("debounces rg grep queries, renders searching indicator, and flushes on Enter", async () => {
    const dir = makeTempWorkspace();
    try {
      const grepCalls: string[] = [];
      let renderRequests = 0;
      const picker = createFilePickerTui(
        {
          requestRender: () => {
            renderRequests += 1;
          },
        },
        plainTheme,
        {
          rootDir: dir,
          initialMode: "grep",
          grepDebounceMs: 35,
          nerdIcons: false,
          runGrep: (_root, query) => {
            grepCalls.push(query);
            return [{ relPath: "src/main.rs", line: 3, column: 9, text: "    let needle_token = 42;" }];
          },
          getRows: () => 50,
        },
        () => {},
      );

      // Tall terminal (50 rows) scales to ~90% height (> 34 old cap).
      expect(picker.render(108).length).toBe(45);

      // Rapid keystrokes should coalesce into a single debounced grep call.
      picker.handleInput("n");
      picker.handleInput("e");
      picker.handleInput("e");
      picker.handleInput("d");
      expect(grepCalls).toHaveLength(0);
      const pendingPlain = picker.render(108).map(stripAnsi).join("\n");
      expect(pendingPlain).toContain("searching… rg");

      await new Promise((r) => setTimeout(r, 60));
      expect(grepCalls).toEqual(["need"]);
      expect(renderRequests).toBeGreaterThanOrEqual(1);

      // Esc cancels any pending debounce timer cleanly.
      picker.handleInput("l");
      picker.handleInput("\x1b");
      await new Promise((r) => setTimeout(r, 60));
      expect(grepCalls).toEqual(["need"]);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("syntax-highlights code preview lines and caches per file", () => {
    const dir = makeTempWorkspace();
    try {
      let highlightCalls = 0;
      const picker = createFilePickerTui(
        { requestRender: () => {} },
        plainTheme,
        {
          rootDir: dir,
          initialMode: "files",
          initialQuery: "main.rs",
          nerdIcons: false,
          highlightFile: (code, relPath) => {
            highlightCalls += 1;
            expect(relPath).toBe("src/main.rs");
            return code.split("\n").map((line) => `\x1b[38;2;97;175;239m${line}\x1b[39m`);
          },
          getRows: () => 28,
        },
        () => {},
      );

      const lines = picker.render(108);
      for (const line of lines) {
        expect(visibleWidth(line)).toBe(108);
      }
      expect(lines.join("\n")).toContain("\x1b[38;2;97;175;239mfn main()");
      expect(highlightCalls).toBe(1);

      // Re-rendering reuses cached highlighted lines.
      picker.render(108);
      expect(highlightCalls).toBe(1);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("coalesces rapid preview-wheel events without extra preview work", async () => {
    let renderRequests = 0;
    let loadPreviewCalls = 0;
    let highlightCalls = 0;
    const picker = createFilePickerTui(
      {
        requestRender: () => {
          renderRequests += 1;
        },
      },
      plainTheme,
      {
        rootDir: "/workspace",
        initialMode: "files",
        initialQuery: "preview.txt",
        nerdIcons: false,
        listFiles: () => [{ relPath: "preview.txt", name: "preview.txt", isDir: false }],
        loadPreview: () => {
          loadPreviewCalls += 1;
          return {
            kind: "file",
            relPath: "preview.txt",
            sizeLabel: "1 KB",
            modified: "2026-01-01 00:00",
            lineCount: 60,
            truncated: false,
            lines: Array.from({ length: 60 }, (_, i) => `line ${i + 1}`),
          };
        },
        highlightFile: (code) => {
          highlightCalls += 1;
          return code.split("\n");
        },
        getRows: () => 28,
      },
      () => {},
    );

    picker.render(108);
    expect(renderRequests).toBe(0);
    expect(loadPreviewCalls).toBe(1);
    expect(highlightCalls).toBe(1);

    const mouse = (overrides: Partial<TuiMouseEvent>): TuiMouseEvent => ({
      type: "wheel",
      button: "none",
      x: 60,
      y: 6,
      screenX: 60,
      screenY: 6,
      width: 108,
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
    expect(loadPreviewCalls).toBe(1);
    expect(highlightCalls).toBe(1);

    await new Promise((resolve) => setTimeout(resolve, 70));
    expect(renderRequests).toBe(1);
    expect(loadPreviewCalls).toBe(1);
    expect(highlightCalls).toBe(1);
  });
});
