import { expect, mock, test } from "bun:test";

import { piTuiMock } from "../pi-grok-remote-tui/test-fixtures.ts";

mock.module("@earendil-works/pi-tui", () => piTuiMock);

test("Rust TUI bridge resolves Pi cursor markers before notifying Pager", async () => {
  const previous = process.env.PI_GROK;
  process.env.PI_GROK = "1";
  let finish: ((result: unknown) => void) | undefined;
  let result: Promise<unknown> | undefined;
  let sessionStart: ((event: unknown, ctx: unknown) => void) | undefined;
  let frame: string[] | undefined;
  const pi = {
    on: (event: string, handler: typeof sessionStart) => {
      if (event === "session_start") sessionStart = handler;
    },
  };
  const ui = {
    custom: async (..._args: any[]): Promise<unknown> => undefined,
    setWidget() {},
    _sendNotification(method: string, params: { op: string; lines?: string[] }) {
      if (method === "pi/ui/remote_tui" && params.op === "frame") frame = params.lines;
    },
  };

  try {
    const { default: registerBridge } = await import("./index.ts");
    registerBridge(pi as never);
    sessionStart?.({}, { ui });
    result = ui.custom((_tui: unknown, _theme: unknown, _kb: unknown, done: typeof finish) => {
      finish = done;
      return {
        invalidate() {},
        render: () => ["before\x1b_pi:c\x07\x1b_pi:fc\x07 \x1b_pi:/fc\x07after"],
      };
    });
    expect(frame?.join("\n")).not.toContain("pi:c");
    expect(frame?.join("\n")).not.toContain("pi:fc");
    expect(frame?.join("\n")).not.toContain("pi:/fc");
    expect(frame).toEqual(["before\x1b[7m \x1b[27mafter"]);
  } finally {
    finish?.(undefined);
    await result;
    if (previous === undefined) delete process.env.PI_GROK;
    else process.env.PI_GROK = previous;
  }
});

