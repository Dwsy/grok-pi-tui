import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, expect, test } from "bun:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import {
  computeSplitGeometry,
  detectNerdFontSupport,
  detectTerminalImageProtocol,
  fitImageCells,
  fitWidth,
  getFileIcon,
  highlightTerms,
  hitRegion,
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
});
