import { afterAll, beforeAll, expect, mock, test } from "bun:test";
import { appendFileSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { TuiMouseEvent } from "@earendil-works/pi-tui";
import { metaPath } from "./transport.ts";

import { piTuiMock } from "./test-fixtures.ts";

mock.module("@earendil-works/pi-tui", () => piTuiMock);

const {
  default: registerRemoteTui,
  createDemoSelector,
  applyDemoCapabilities,
} = await import("./index.ts");
const { dispatchComponentInput, installCustomPatch } = await import("./host.ts");
const testDirectory = mkdtempSync(join(tmpdir(), "remote-tui-host-tests-"));
const previousMetaPath = process.env.PI_GROK_REMOTE_TUI_META;
beforeAll(() => { process.env.PI_GROK_REMOTE_TUI_META = join(testDirectory, "active.json"); });
afterAll(async () => {
  // Close the final test's host and watcher before releasing the isolated path.
  const ui = { custom: async (..._args: any[]): Promise<any> => undefined, setWidget() {} };
  installCustomPatch(ui);
  await ui.custom((_tui: unknown, _theme: unknown, _kb: unknown, done: (result: unknown) => void) => {
    done(undefined);
    return { render: () => [], invalidate() {} };
  });
  if (previousMetaPath === undefined) delete process.env.PI_GROK_REMOTE_TUI_META;
  else process.env.PI_GROK_REMOTE_TUI_META = previousMetaPath;
  rmSync(testDirectory, { recursive: true });
});

test("focused custom child gets letters and Escape without global shortcut interception", async () => {
  const inputs: string[] = [];
  let rootFocus = false;
  let childFocus = false;
  const globals = globalThis as typeof globalThis & { __piGrokShortcutIntercept?: (data: string) => boolean };
  const previous = globals.__piGrokShortcutIntercept;
  let intercepted = 0;
  globals.__piGrokShortcutIntercept = () => { intercepted++; return true; };
  const ui = { custom: async (..._args: any[]): Promise<any> => undefined, setWidget() {} };
  installCustomPatch(ui);
  let finish!: (result: string) => void;
  let showOverlay!: (component: any) => { hide(): void };
  try {
    const result = ui.custom((tui: any, _theme: unknown, _kb: unknown, done: typeof finish) => {
      finish = done;
      showOverlay = tui.showOverlay;
      tui.setFocus({
        invalidate() {}, render: () => [],
        get focused() { return childFocus; },
        set focused(value: boolean) { childFocus = value; },
        handleInput(data: string) { inputs.push(data); if (data === "s") done("saved"); },
      });
      return {
        invalidate() {}, render: () => ["frame"],
        get focused() { return rootFocus; },
        set focused(value: boolean) { rootFocus = value; },
        handleInput() { throw new Error("root must not steal child focus"); },
      };
    });
    await new Promise((resolve) => setImmediate(resolve));
    expect(childFocus).toBe(true);
    expect(rootFocus).toBe(false);
    const overlay = showOverlay({ invalidate() {}, render: () => ["nested"] });
    expect(childFocus).toBe(false);
    overlay.hide();
    expect(childFocus).toBe(true);
    const { id, keysPath } = JSON.parse(readFileSync(metaPath(), "utf8"));
    appendFileSync(keysPath, ["a", "\x1b", "s"].map((data) => JSON.stringify({ id, op: "input", data }) + "\n").join(""));
    expect(await result).toBe("saved");
    expect(inputs).toEqual(["a", "\x1b", "s"]);
    expect(intercepted).toBe(0);
    expect(childFocus).toBe(false);
  } finally {
    finish?.("cancelled");
    globals.__piGrokShortcutIntercept = previous;
  }
});

test("remote custom host dispatches clicks, captured drags, modifiers and wheel deltas", async () => {
  const mouseEvents: TuiMouseEvent[] = [];
  const inputs: string[] = [];
  const ui = { custom: async (..._args: any[]): Promise<any> => undefined, setWidget() {} };
  installCustomPatch(ui);
  let finish!: (result: string) => void;
  const result = ui.custom((_tui: unknown, _theme: unknown, _kb: unknown, done: typeof finish) => {
    finish = done;
    return {
      invalidate() {},
      render: () => ["frame"],
      focused: false,
      handleInput: (data: string) => inputs.push(data),
      handleMouse: (event: TuiMouseEvent) => {
        mouseEvents.push(event);
        if (event.type === "press") return { handled: true, capture: true, focus: true };
        if (event.type === "release") return { handled: true, render: false };
        return { handled: true };
      },
    };
  });
  try {
    await new Promise((resolve) => setImmediate(resolve));
    const { id, keysPath } = JSON.parse(readFileSync(metaPath(), "utf8"));
    const event = (
      type: TuiMouseEvent["type"],
      x: number,
      y: number,
      screenX: number,
      screenY: number,
      extra: Partial<TuiMouseEvent> = {},
    ): TuiMouseEvent => ({
      type,
      button: type === "wheel" || type === "move" ? "none" : "left",
      x,
      y,
      screenX,
      screenY,
      width: 80,
      height: 4,
      shift: false,
      alt: false,
      ctrl: false,
      ...extra,
    });
    appendFileSync(keysPath, [
      { id, op: "mouse", event: event("press", 3, 1, 5, 2, { shift: true }) },
      { id, op: "mouse", event: event("release", 3, 1, 5, 2, { shift: true }) },
      { id, op: "mouse", event: event("press", 3, 1, 5, 2) },
      { id, op: "mouse", event: event("release", 3, 1, 5, 2) },
      { id, op: "mouse", event: event("press", 3, 1, 5, 2) },
      { id, op: "mouse", event: event("drag", 90, 9, 92, 10) },
      { id, op: "mouse", event: event("release", 90, 9, 92, 10) },
      { id, op: "mouse", event: event("wheel", 3, 1, 5, 2, { wheelDelta: -1, alt: true }) },
      { id, op: "mouse", event: event("press", 4, 1, 6, 2, { button: "middle" }) },
      { id, op: "mouse", event: event("release", 4, 1, 6, 2, { button: "middle" }) },
      { id, op: "mouse", event: event("press", 7, 1, 9, 2, { button: "right" }) },
      { id, op: "mouse", event: event("release", 7, 1, 9, 2, { button: "right" }) },
      { id, op: "mouse", event: event("wheel", 3, 1, 5, 2, { wheelDelta: 1 }) },
    ].map((entry) => JSON.stringify(entry) + "\n").join(""));
    await new Promise((resolve) => setTimeout(resolve, 100));

    expect(mouseEvents.map((entry) => entry.type)).toEqual([
      "press", "release", "click", "press", "release", "click", "press", "drag", "release", "wheel",
      "press", "release", "click", "press", "release", "click", "wheel",
    ]);
    expect(mouseEvents[0]).toMatchObject({ x: 3, y: 1, screenX: 5, screenY: 2, shift: true });
    expect(mouseEvents[2]?.clickCount).toBe(1);
    expect(mouseEvents[5]?.clickCount).toBe(2);
    expect(mouseEvents[7]).toMatchObject({ x: 90, y: 9, width: 80, height: 4 });
    expect(mouseEvents[9]?.wheelDelta).toBe(-5);
    expect(mouseEvents[10]).toMatchObject({ type: "press", button: "middle" });
    expect(mouseEvents[12]).toMatchObject({ type: "click", button: "middle", clickCount: 1 });
    expect(mouseEvents[13]).toMatchObject({ type: "press", button: "right" });
    expect(mouseEvents[15]).toMatchObject({ type: "click", button: "right", clickCount: 1 });
    expect(mouseEvents[16]?.wheelDelta).toBe(1);
    expect(inputs).toEqual([]);
  } finally {
    finish?.("done");
    expect(await result).toBe("done");
  }
});

test("async custom factories claim input before rendering and publish matching close", async () => {
  const lifecycle: { op: string; id: string }[] = [];
  const ui = {
    custom: async (..._args: any[]): Promise<any> => undefined,
    setWidget: (key: string, lines?: string[]) => {
      if (key === "__pi_grok_remote_tui_session__" && lines?.[0]) {
        lifecycle.push(JSON.parse(lines[0]));
      }
    },
  };
  installCustomPatch(ui);
  let finish!: (result: string) => void;
  const result = ui.custom((_tui: unknown, _theme: unknown, _kb: unknown, done: typeof finish) => {
    finish = done;
    return { render: () => ["frame"], invalidate() {} };
  });
  expect(lifecycle).toHaveLength(1);
  expect(lifecycle[0]?.op).toBe("open");
  await new Promise((resolve) => setImmediate(resolve));
  finish("saved");
  expect(await result).toBe("saved");
  expect(lifecycle.at(-1)).toEqual({ op: "close", id: lifecycle[0]!.id });
});

test("remote host filters key release unless component opts in", () => {
  const regularInputs: string[] = [];
  const releaseAwareInputs: string[] = [];
  const press = "\x1b[B";
  const release = "\x1b[1;1:3B";

  dispatchComponentInput(
    {
      invalidate() {},
      render: () => [],
      handleInput: (data) => regularInputs.push(data),
    },
    press,
  );
  dispatchComponentInput(
    {
      invalidate() {},
      render: () => [],
      handleInput: (data) => regularInputs.push(data),
    },
    release,
  );
  dispatchComponentInput(
    {
      wantsKeyRelease: true,
      invalidate() {},
      render: () => [],
      handleInput: (data) => releaseAwareInputs.push(data),
    },
    release,
  );

  expect(regularInputs).toEqual([press]);
  expect(releaseAwareInputs).toEqual([release]);
});

test("custom host is NOT installed under native Pi (no PI_GROK)", async () => {
  const previousGrok = process.env.PI_GROK;
  const previousFlag = process.env.PI_GROK_REMOTE_TUI;
  delete process.env.PI_GROK;
  delete process.env.PI_GROK_REMOTE_TUI;

  let sessionStart:
    | ((event: unknown, ctx: { ui: { custom: (...args: unknown[]) => unknown; setWidget: () => void } }) => void)
    | undefined;
  const pi = {
    on: (_event: string, handler: typeof sessionStart) => {
      sessionStart = handler;
    },
    registerCommand: () => {},
  };
  const originalCustom = async () => "native";
  const ui = {
    custom: originalCustom,
    setWidget: () => {},
  };

  try {
    registerRemoteTui(pi as never);
    sessionStart?.({}, { ui });
    expect(ui.custom).toBe(originalCustom);
    expect(await ui.custom()).toBe("native");
  } finally {
    if (previousGrok === undefined) delete process.env.PI_GROK;
    else process.env.PI_GROK = previousGrok;
    if (previousFlag === undefined) delete process.env.PI_GROK_REMOTE_TUI;
    else process.env.PI_GROK_REMOTE_TUI = previousFlag;
  }
});

test("custom host exposes terminal dimensions to component factories", async () => {
  const previous = process.env.PI_GROK_REMOTE_TUI;
  process.env.PI_GROK_REMOTE_TUI = "1";

  let sessionStart:
    | ((event: unknown, ctx: { ui: { custom: (...args: unknown[]) => unknown; setWidget: () => void } }) => void)
    | undefined;
  const pi = {
    on: (_event: string, handler: typeof sessionStart) => {
      sessionStart = handler;
    },
    registerCommand: () => {},
  };
  const ui = {
    custom: async () => undefined,
    setWidget: () => {},
  };

  try {
    registerRemoteTui(pi as never);
    sessionStart?.({}, { ui });

    const result = await ui.custom((tui: { terminal: { columns: number; rows: number } }, _theme, _kb, done) => {
      expect(tui.terminal.columns).toBeGreaterThan(0);
      expect(tui.terminal.rows).toBeGreaterThan(0);
      done("ok");
      return { invalidate() {}, render: () => [], handleInput() {} };
    });

    expect(result).toBe("ok");
  } finally {
    if (previous === undefined) delete process.env.PI_GROK_REMOTE_TUI;
    else process.env.PI_GROK_REMOTE_TUI = previous;
  }
});

test("custom host mirrors Pi overlay width and position metadata", async () => {
  const previous = process.env.PI_GROK_REMOTE_TUI;
  const previousWidth = process.env.PI_GROK_REMOTE_TUI_WIDTH;
  process.env.PI_GROK_REMOTE_TUI = "1";
  process.env.PI_GROK_REMOTE_TUI_WIDTH = "80";

  let sessionStart:
    | ((event: unknown, ctx: { ui: { custom: (...args: unknown[]) => unknown; setWidget: (key: string, lines?: string[]) => void } }) => void)
    | undefined;
  const pi = {
    on: (_event: string, handler: typeof sessionStart) => {
      sessionStart = handler;
    },
    registerCommand: () => {},
  };
  let layout: Record<string, unknown> | undefined;
  const ui = {
    custom: async () => undefined,
    setWidget: (key: string, lines?: string[]) => {
      if (key === "__pi_grok_remote_tui_layout__" && lines?.[0]) {
        layout = JSON.parse(lines[0]) as Record<string, unknown>;
      }
    },
  };

  try {
    registerRemoteTui(pi as never);
    sessionStart?.({}, { ui });
    void ui.custom(
      (_tui, _theme, _kb, _done) => ({ invalidate() {}, render: () => ["frame"], handleInput() {} }),
      {
        overlay: true,
        overlayOptions: {
          width: "50%",
          maxHeight: "60%",
          anchor: "top-left",
          offsetX: 2,
          offsetY: 3,
        },
      },
    );
    await new Promise((resolve) => setImmediate(resolve));
    expect(layout).toMatchObject({
      overlay: true,
      width: 40,
      maxHeight: "60%",
      anchor: "top-left",
      offsetX: 2,
      offsetY: 3,
    });
  } finally {
    if (previous === undefined) delete process.env.PI_GROK_REMOTE_TUI;
    else process.env.PI_GROK_REMOTE_TUI = previous;
    if (previousWidth === undefined) delete process.env.PI_GROK_REMOTE_TUI_WIDTH;
    else process.env.PI_GROK_REMOTE_TUI_WIDTH = previousWidth;
  }
});

test("custom host keeps Pi custom inline by default", async () => {
  const previous = process.env.PI_GROK_REMOTE_TUI;
  const previousWidth = process.env.PI_GROK_REMOTE_TUI_WIDTH;
  process.env.PI_GROK_REMOTE_TUI = "1";
  process.env.PI_GROK_REMOTE_TUI_WIDTH = "120";

  let sessionStart:
    | ((event: unknown, ctx: { ui: { custom: (...args: unknown[]) => unknown; setWidget: (key: string, lines?: string[]) => void } }) => void)
    | undefined;
  const pi = {
    on: (_event: string, handler: typeof sessionStart) => {
      sessionStart = handler;
    },
    registerCommand: () => {},
  };
  let layout: Record<string, unknown> | undefined;
  const ui = {
    custom: async () => undefined,
    setWidget: (key: string, lines?: string[]) => {
      if (key === "__pi_grok_remote_tui_layout__" && lines?.[0]) {
        layout = JSON.parse(lines[0]) as Record<string, unknown>;
      }
    },
  };

  try {
    registerRemoteTui(pi as never);
    sessionStart?.({}, { ui });
    void ui.custom((_tui, _theme, _kb, _done) => ({
      width: 37,
      invalidate() {},
      render: () => ["frame"],
      handleInput() {},
    }));
    await new Promise((resolve) => setImmediate(resolve));
    expect(layout).toMatchObject({ overlay: false, width: 120 });
  } finally {
    if (previous === undefined) delete process.env.PI_GROK_REMOTE_TUI;
    else process.env.PI_GROK_REMOTE_TUI = previous;
    if (previousWidth === undefined) delete process.env.PI_GROK_REMOTE_TUI_WIDTH;
    else process.env.PI_GROK_REMOTE_TUI_WIDTH = previousWidth;
  }
});

test("custom host resolves Pi hardware and fake cursor markers in projected frames", async () => {
  const previous = process.env.PI_GROK_REMOTE_TUI;
  process.env.PI_GROK_REMOTE_TUI = "1";

  let sessionStart:
    | ((event: unknown, ctx: { ui: { custom: (...args: unknown[]) => unknown; setWidget: (key: string, lines?: string[]) => void } }) => void)
    | undefined;
  const pi = {
    on: (_event: string, handler: typeof sessionStart) => {
      sessionStart = handler;
    },
    registerCommand: () => {},
  };
  let frame: string[] | undefined;
  const ui = {
    custom: async () => undefined,
    setWidget: (_key: string, lines?: string[]) => {
      frame = lines;
    },
  };

  try {
    registerRemoteTui(pi as never);
    sessionStart?.({}, { ui });
    void ui.custom((_tui, _theme, _kb, _done) => ({
      invalidate() {},
      render: () => [
        "before\x1b_pi:c\x07\x1b_pi:fc\x07 \x1b_pi:/fc\x07after\x1b_pi:fc\x07X\x1b_pi:/fc\x07",
        "\x1b[31malready\x1b[7m \x1b[27mresolved\x1b[39m",
      ],
      handleInput() {},
    }));
    await new Promise((resolve) => setImmediate(resolve));
    await new Promise((resolve) => setImmediate(resolve));

    expect(frame).toBeDefined();
    expect(frame?.join("\n")).not.toContain("pi:c");
    expect(frame?.join("\n")).not.toContain("pi:fc");
    expect(frame?.join("\n")).not.toContain("pi:/fc");
    expect(frame).toEqual([
      "before\x1b[7m \x1b[27mafter\x1b[7mX\x1b[27m",
      "\x1b[31malready\x1b[7m \x1b[27mresolved\x1b[39m",
    ]);
  } finally {
    if (previous === undefined) delete process.env.PI_GROK_REMOTE_TUI;
    else process.env.PI_GROK_REMOTE_TUI = previous;
  }
});

test("demo SettingsList toggles and applies selected surfaces", () => {
  const applied: string[][] = [];
  let closed: string | undefined;
  const theme = {
    fg: (_c: string, text: string) => text,
    bold: (text: string) => text,
  };
  const demo = createDemoSelector(
    { requestRender: () => {} },
    theme,
    (result) => {
      closed = result;
    },
    (keys) => {
      applied.push([...keys]);
    },
  );

  // Space on first item (header) → on
  demo.handleInput?.(" ");
  // Move down and enable footer
  demo.handleInput?.("\x1b[B");
  demo.handleInput?.(" ");
  expect(applied).toEqual([["header"], ["header", "footer"]]);

  const rendered = demo.render(80).join("\n");
  expect(rendered).toContain("Remote TUI capability lab");
  expect(rendered).toContain("Header widget=on");
  expect(rendered).toContain("Footer widget=on");

  // Esc closes with selected keys
  demo.handleInput?.("\x1b");
  expect(closed).toBe("header,footer");
});

test("applyDemoCapabilities projects header/footer/status/title/editor", () => {
  const widgets = new Map<string, { lines?: string[]; placement?: string }>();
  let status: { key?: string; text?: string } = {};
  let title: string | undefined;
  let editorText: string | undefined;

  applyDemoCapabilities(
    {
      setWidget: (key, lines, options) => {
        widgets.set(key, { lines, placement: options?.placement });
      },
      setStatus: (key, text) => {
        status = { key, text };
      },
      setTitle: (value) => {
        title = value;
      },
      setEditorText: (value) => {
        editorText = value;
      },
    },
    ["header", "footer", "status", "title", "editor"],
  );

  expect(widgets.get("remote_tui_demo_header")?.placement).toBe("aboveEditor");
  expect(widgets.get("remote_tui_demo_header")?.lines?.join("\n")).toContain("Remote TUI demo header");
  expect(widgets.get("remote_tui_demo_footer")?.placement).toBe("belowEditor");
  expect(widgets.get("remote_tui_demo_footer")?.lines?.join("\n")).toContain("Footer · 5 selected");
  expect(widgets.get("remote_tui_demo_footer")?.lines?.join("\n")).not.toContain("Esc");
  expect(status).toEqual({
    key: "remote-tui-demo",
    text: "Remote TUI demo: Header widget, Footer widget, Status bar, Window title, Prompt editor",
  });
  expect(title).toBe("Remote TUI capability lab");
  expect(editorText).toContain("Remote TUI demo applied");
});

test("showOverlay restores previous root component on hide", async () => {
  const previous = process.env.PI_GROK_REMOTE_TUI;
  process.env.PI_GROK_REMOTE_TUI = "1";

  let sessionStart:
    | ((event: unknown, ctx: { ui: { custom: (...args: unknown[]) => unknown; setWidget: (key: string, lines?: string[]) => void } }) => void)
    | undefined;
  const pi = {
    on: (_event: string, handler: typeof sessionStart) => {
      sessionStart = handler;
    },
    registerCommand: () => {},
  };
  const ui = {
    custom: async () => undefined,
    setWidget: () => {},
  };

  try {
    registerRemoteTui(pi as never);
    sessionStart?.({}, { ui });

    let tuiRef: {
      showOverlay: (component: {
        invalidate(): void;
        render(width: number): string[];
        handleInput?(data: string): void;
      }) => { hide: () => void };
    } | null = null;

    void ui.custom((tui, _theme, _kb, _done) => {
      tuiRef = tui as typeof tuiRef;
      return {
        invalidate() {},
        render: () => ["root"],
        handleInput() {},
      };
    });
    await new Promise((resolve) => setImmediate(resolve));
    await new Promise((resolve) => setImmediate(resolve));

    const handle = tuiRef!.showOverlay({
      invalidate() {},
      render: () => ["overlay"],
      handleInput() {},
    });
    handle.hide();
    expect(tuiRef).toBeTruthy();
  } finally {
    if (previous === undefined) delete process.env.PI_GROK_REMOTE_TUI;
    else process.env.PI_GROK_REMOTE_TUI = previous;
  }
});
