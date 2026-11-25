import { describe, expect, test } from "bun:test";
import { visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { createGoalDetailTui } from "./goal-detail-tui.ts";

const plainTheme = {
  fg: (_color: string, text: string) => text,
  bold: (text: string) => text,
};

function stripAnsi(text: string): string {
  return text.replace(/\x1b\[[0-9;]*m/g, "");
}

describe("GoalDetailTui", () => {
  test("renders goal overview + history, supports navigation, divider drag, and g/Esc close", () => {
    let closed = false;
    const tui = createGoalDetailTui(
      { requestRender: () => {} },
      plainTheme,
      {
        goal: {
          goalId: "goal-42",
          objective: "Migrate goal_detail to TS Remote TUI cleanly",
          status: "active",
          phase: "executing",
          tokensUsed: 12_500,
          createdAt: "2026-11-25T12:00:00Z",
        },
        history: [
          {
            timestamp: "2026-11-25T12:00:00Z",
            event: "goal_created",
            status: "active",
            detail: "Initial goal created",
          },
          {
            timestamp: "2026-11-25T12:05:00Z",
            event: "progress",
            status: "active",
            detail: "Wired Rust dispatch + TS overlay",
          },
        ],
        getRows: () => 24,
      },
      () => {
        closed = true;
      },
    );

    const width = 96;
    const lines = tui.render(width);
    for (const line of lines) expect(visibleWidth(line)).toBe(width);
    const plain = lines.map(stripAnsi).join("\n");
    expect(plain).toContain("Goal Detail");
    expect(plain).toContain("Migrate goal_detail to TS Remote TUI cleanly");
    expect(plain).toContain("12.5k tok");

    // Down selects latest history event.
    tui.handleInput("\x1b[B");
    expect(tui.getSelectedSectionId()).toBe("hist-0");
    expect(tui.render(width).map(stripAnsi).join("\n")).toContain("Wired Rust dispatch + TS overlay");

    // Dragging divider resizes split cleanly.
    const dividerX = stripAnsi(lines[2]!).indexOf("┬");
    expect(dividerX).toBeGreaterThan(20);
    const makeMouse = (type: TuiMouseEvent["type"], x: number): TuiMouseEvent => ({
      type,
      button: type === "release" ? "none" : "left",
      x,
      y: 6,
      screenX: x,
      screenY: 6,
      width,
      height: lines.length,
      shift: false,
      alt: false,
      ctrl: false,
    });
    expect(tui.handleMouse(makeMouse("press", dividerX))?.capture).toBe(true);
    tui.handleMouse(makeMouse("drag", 52));
    tui.handleMouse(makeMouse("release", 52));
    expect(stripAnsi(tui.render(width)[2]!).indexOf("┬")).toBeGreaterThan(dividerX);

    // Pressing `g` closes GoalDetailTui (matches Rust toggle key).
    tui.handleInput("g");
    expect(closed).toBe(true);
  });
});
