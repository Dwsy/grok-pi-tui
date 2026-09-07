import { expect, mock, test } from "bun:test";

type SettingItem = {
  id: string;
  label: string;
  description?: string;
  currentValue: string;
  values?: string[];
};

/** Minimal SettingsList stand-in: space/enter cycles, esc cancels. */
class MockSettingsList {
  private items: SettingItem[];
  private index = 0;
  private onChange: (id: string, newValue: string) => void;
  private onCancel: () => void;

  constructor(
    items: SettingItem[],
    _maxVisible: number,
    _theme: unknown,
    onChange: (id: string, newValue: string) => void,
    onCancel: () => void,
  ) {
    this.items = items;
    this.onChange = onChange;
    this.onCancel = onCancel;
  }

  invalidate() {}
  render() {
    return this.items.map((item) => `${item.label}=${item.currentValue}`);
  }

  handleInput(data: string) {
    if (data === "\x1b[A") {
      this.index = this.index === 0 ? this.items.length - 1 : this.index - 1;
      return;
    }
    if (data === "\x1b[B") {
      this.index = this.index === this.items.length - 1 ? 0 : this.index + 1;
      return;
    }
    if (data === " " || data === "\r") {
      const item = this.items[this.index]!;
      const values = item.values ?? ["on", "off"];
      const next = values[(values.indexOf(item.currentValue) + 1) % values.length]!;
      item.currentValue = next;
      this.onChange(item.id, next);
      return;
    }
    if (data === "\x1b") {
      this.onCancel();
    }
  }
}

mock.module("@earendil-works/pi-tui", () => ({
  CURSOR_MARKER: "\x1b_pi:c\x07",
  isKeyRelease: (data: string) => /:3(?:u|~|[ABCDHF])/.test(data),
  KeybindingsManager: class {
    matches() {
      return false;
    }
  },
  TUI_KEYBINDINGS: {},
  setKeybindings: () => {},
  SettingsList: MockSettingsList,
}));

const {
  default: registerRemoteTui,
  createDemoSelector,
  applyDemoCapabilities,
} = await import("./index.ts");
const { dispatchComponentInput } = await import("./host.ts");

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

test("custom host removes Pi hardware cursor markers from projected frames", async () => {
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
      render: () => ["before\x1b_pi:c\x07\x1b[7m \x1b[27mafter"],
      handleInput() {},
    }));
    await new Promise((resolve) => setImmediate(resolve));
    await new Promise((resolve) => setImmediate(resolve));

    expect(frame).toBeDefined();
    expect(frame?.join("\n")).not.toContain("pi:c");
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
