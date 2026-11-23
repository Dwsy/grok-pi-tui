import { describe, expect, test } from "bun:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import {
  computeSplitGeometry,
  detectNerdFontSupport,
  fitWidth,
  getFileIcon,
  highlightTerms,
  hitRegion,
  renderSplitOverlayFrame,
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
});
