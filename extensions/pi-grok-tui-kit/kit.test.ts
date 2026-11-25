import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, expect, test } from "bun:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import {
  computeSplitGeometry,
  detectNerdFontSupport,
  detectTerminalImageProtocol,
  DividerDragController,
  fitImageCells,
  fitWidth,
  getFileIcon,
  highlightTerms,
  hitRegion,
  ListController,
  RenderCoalescer,
  probeImageMetadata,
  renderSplitOverlayFrame,
  rgb24ToHalfBlockLines,
  wrapTextToWidth,
  type TuiTheme,
} from "./index.ts";

const plainTheme: TuiTheme = {
  fg: (_c, text) => text,
  bold: (text) => text,
};

describe("pi-grok-tui-kit shared primitives", () => {
  test("fitWidth and wrapTextToWidth respect CJK double-width columns", () => {
    expect(visibleWidth(fitWidth("中文测试", 10))).toBe(10);
    expect(visibleWidth(fitWidth("中文测试超长内容", 6))).toBe(6);
    expect(wrapTextToWidth("中文测试AB", 4)).toEqual(["中文", "测试", "AB"]);
  });

  test("highlightTerms highlights multi-term queries", () => {
    expect(highlightTerms("Hello World", "hello world", (m) => `[${m}]`)).toBe("[Hello] [World]");
  });

  test("highlightTerms skips ANSI CSI/OSC escape sequences when highlighting", () => {
    const styled = "\x1b[38;2;121;192;255mError 38 in \x1b[3m3m module\x1b[23m\x1b[39m";
    const out = highlightTerms(styled, "38 3m", (m) => `[${m}]`);
    expect(out).toBe("\x1b[38;2;121;192;255mError [38] in \x1b[3m[3m] module\x1b[23m\x1b[39m");
  });

  test("renderSplitOverlayFrame builds aligned borders and accurate pill click regions", () => {
    const geometry = computeSplitGeometry({ width: 90, rows: 24, hasSearchBar: true });
    const leftLines = Array.from({ length: geometry.bodyHeight }, () =>
      fitWidth(" left", geometry.leftWidth),
    );
    const rightLines = Array.from({ length: geometry.bodyHeight }, () =>
      fitWidth(" right", geometry.rightWidth),
    );

    const frame = renderSplitOverlayFrame({
      geometry,
      titlePrefix: " Demo  ",
      pills: [
        { id: "a", label: "[Alpha]", active: true },
        { id: "b", label: "[Beta]", active: false },
      ],
      search: {
        promptPrefix: " > ",
        query: "",
        placeholder: "search...",
        rightLabel: "0 items ",
      },
      leftLines,
      rightLines,
      footerHints: [
        { key: "Tab", label: "switch" },
        { key: "Esc", label: "close" },
      ],
      theme: plainTheme,
    });

    expect(frame.lines[0]!.startsWith("╭")).toBe(true);
    expect(frame.lines[frame.lines.length - 1]!.endsWith("╯")).toBe(true);
    for (const line of frame.lines) {
      expect(visibleWidth(line)).toBe(90);
    }

    const alphaRect = frame.pillRects.get("a")!;
    expect(hitRegion(alphaRect, alphaRect.x1 + 1, 1)).toBe(true);
    expect(hitRegion(frame.closeRect, frame.closeRect.x1, 1)).toBe(true);
  });

  test("detectNerdFontSupport and getFileIcon resolve devicons and ASCII fallback", () => {
    expect(detectNerdFontSupport({ NERD_FONT: "1" })).toBe(true);
    expect(detectNerdFontSupport({ NERD_FONT: "0" })).toBe(false);
    expect(getFileIcon("lib.rs", { nerdIcons: true }).glyph).toBe("");
    expect(getFileIcon("lib.rs", { nerdIcons: false }).glyph).toBe("·");
  });

  test("detectTerminalImageProtocol recognizes Herdr, Kitty, Ghostty, WezTerm, iTerm2, and fallback", () => {
    expect(detectTerminalImageProtocol({ HERDR_ENV: "1" })).toEqual({
      protocol: "kitty",
      label: "HD · Herdr/Kitty",
    });
    expect(detectTerminalImageProtocol({ TERM_PROGRAM: "herdr" }).protocol).toBe("kitty");
    expect(detectTerminalImageProtocol({ TERM_PROGRAM: "ghostty" }).protocol).toBe("kitty");
    expect(detectTerminalImageProtocol({ TERM_PROGRAM: "WezTerm" }).protocol).toBe("kitty");
    expect(detectTerminalImageProtocol({ TERM_PROGRAM: "iTerm.app" }).protocol).toBe("iterm2");
    expect(detectTerminalImageProtocol({ TERM: "xterm-256color" }).protocol).toBe("halfblock");
  });

  test("probeImageMetadata parses PNG/SVG headers and rgb24ToHalfBlockLines encodes truecolor rows", () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-grok-kit-img-"));
    try {
      const pngPath = path.join(dir, "sample.png");
      // Valid PNG IHDR: 640x480
      const pngBuf = Buffer.alloc(33);
      Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]).copy(pngBuf, 0);
      pngBuf.writeUInt32BE(13, 8);
      pngBuf.write("IHDR", 12, "ascii");
      pngBuf.writeUInt32BE(640, 16);
      pngBuf.writeUInt32BE(480, 20);
      fs.writeFileSync(pngPath, pngBuf);

      const pngMeta = probeImageMetadata(pngPath);
      expect(pngMeta?.format).toBe("PNG");
      expect(pngMeta?.width).toBe(640);
      expect(pngMeta?.height).toBe(480);

      const svgPath = path.join(dir, "logo.svg");
      fs.writeFileSync(svgPath, '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 64"></svg>');
      const svgMeta = probeImageMetadata(svgPath);
      expect(svgMeta?.format).toBe("SVG");
      expect(svgMeta?.width).toBe(128);
      expect(svgMeta?.height).toBe(64);

      const fit = fitImageCells(640, 480, 40, 16);
      expect(fit.cols).toBeLessThanOrEqual(40);
      expect(fit.rows).toBeLessThanOrEqual(16);

      // 2x2 pixels -> 1 row of 2 upper half-blocks
      const rgb = Buffer.from([255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0]);
      const half = rgb24ToHalfBlockLines(rgb, 2, 2);
      expect(half.length).toBe(1);
      expect(half[0]).toContain("\x1b[38;2;255;0;0m\x1b[48;2;0;0;255m▀");
      expect(visibleWidth(half[0]!)).toBe(2);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  test("ListController tracks selection, viewport scroll, and double-click confirmation", () => {
    const list = new ListController({ rowsPerItem: 2, doubleClickMs: 400 });
    expect(list.itemsPerPage(10)).toBe(5);
    list.move(6, 12, 10);
    expect(list.selected).toBe(6);
    expect(list.scroll).toBe(2);
    expect(list.indexAtBodyRow(2, 12)).toBe(3);

    expect(list.handleClick(3, 12, 10, 1000)).toBe("select");
    expect(list.handleClick(3, 12, 10, 1250)).toBe("confirm");
    expect(list.handleClick(3, 12, 10, 1300)).toBe("select");
  });

  test("RenderCoalescer merges rapid schedule calls and supports cancel", async () => {
    let renders = 0;
    const coalescer = new RenderCoalescer(() => { renders += 1; }, 20);
    coalescer.schedule();
    coalescer.schedule();
    coalescer.schedule();
    expect(coalescer.pending).toBe(true);
    expect(renders).toBe(0);
    await new Promise((r) => setTimeout(r, 45));
    expect(renders).toBe(1);
    expect(coalescer.pending).toBe(false);

    coalescer.schedule();
    coalescer.cancel();
    await new Promise((r) => setTimeout(r, 45));
    expect(renders).toBe(1);
  });

  test("DividerDragController handles press, clamped drag, and release in memory", () => {
    const drag = new DividerDragController(0.5);
    const bounds = { totalWidth: 100, dividerX: 50, bodyStartY: 3, bodyHeight: 15, minLeft: 20, minRight: 28 };
    expect(drag.computeLeftWidth(100, 20, 28)).toBe(49);
    expect(drag.handleMouse({ type: "press", button: "left", x: 50, y: 8 }, bounds)).toEqual({
      handled: true,
      capture: true,
      render: true,
    });
    expect(drag.dragging).toBe(true);
    drag.handleMouse({ type: "drag", button: "left", x: 5, y: 8 }, bounds);
    expect(drag.computeLeftWidth(100, 20, 28)).toBe(20);
    drag.handleMouse({ type: "drag", button: "left", x: 200, y: 8 }, bounds);
    expect(drag.computeLeftWidth(100, 20, 28)).toBe(71);
    drag.handleMouse({ type: "release", button: "none", x: 71, y: 8 }, bounds);
    expect(drag.dragging).toBe(false);
  });

  test("every bundled TS extension module is embedded in its Rust injector", () => {
    const repoRoot = path.resolve(import.meta.dir, "..", "..");
    const injectorRoot = path.join(repoRoot, "crates/codegen/xai-grok-pager-bin/src/bin/grok_pi");
    const pairs: Array<{ extDir: string; rustFile: string; skip?: Set<string> }> = [
      { extDir: "extensions/pi-grok-tui-kit", rustFile: "tui_kit.rs" },
      { extDir: "extensions/pi-grok-mentions", rustFile: "mentions_extension.rs" },
      { extDir: "extensions/pi-grok-tree", rustFile: "tree_extension.rs" },
      { extDir: "extensions/pi-grok-review", rustFile: "review_extension.rs" },
      { extDir: "extensions/pi-grok-config", rustFile: "config_extension.rs" },
      { extDir: "extensions/pi-grok-goal", rustFile: "goal_extension.rs" },
      {
        extDir: "extensions/pi-grok-remote-tui",
        rustFile: "remote_tui_extension.rs",
        skip: new Set(["demo.ts", "test-fixtures.ts"]),
      },
    ];
    for (const { extDir, rustFile, skip } of pairs) {
      const rustSrc = fs.readFileSync(path.join(injectorRoot, rustFile), "utf8");
      const tsFiles = fs
        .readdirSync(path.join(repoRoot, extDir))
        .filter((f) => f.endsWith(".ts") && !f.endsWith(".test.ts") && !skip?.has(f));
      for (const file of tsFiles) {
        expect(rustSrc).toContain(`/${file}"`);
      }
    }
  });
});
