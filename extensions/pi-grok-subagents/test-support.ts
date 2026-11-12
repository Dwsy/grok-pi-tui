/**
 * Test-only helpers: a fake `ctx.ui.custom` host that runs pi-tui component
 * factories in-process so tests can drive keyboard and mouse input directly.
 */

import type { Component, TuiMouseEvent, TuiMouseEventType } from "@earendil-works/pi-tui";

export type OpenedPanel = {
  component: Component;
  renderText: (width?: number) => string;
  keys: (data: string) => void;
  mouse: (event: Partial<TuiMouseEvent> & { type: TuiMouseEventType }) => void;
  click: (x: number, y: number) => void;
  wheel: (x: number, y: number, wheelDelta: number) => void;
  resolved: boolean;
};

export type FakeCustomUi = {
  ui: {
    custom: (factory: (tui: unknown, theme: unknown, keyboard: unknown, done: (value: unknown) => void) => Component) => Promise<unknown>;
    notify: (message: string, kind?: string) => void;
  };
  opened: OpenedPanel[];
  notifications: Array<{ message: string; kind?: string }>;
};

function mouseEvent(input: Partial<TuiMouseEvent> & { type: TuiMouseEventType }): TuiMouseEvent {
  return {
    button: "left",
    x: 0,
    y: 0,
    screenX: 0,
    screenY: 0,
    width: 100,
    height: 40,
    shift: false,
    alt: false,
    ctrl: false,
    ...(input.type === "click" ? { clickCount: 1 } : {}),
    ...input,
  };
}

export function fakeCustomUi(): FakeCustomUi {
  const opened: OpenedPanel[] = [];
  const notifications: Array<{ message: string; kind?: string }> = [];
  const ui = {
    custom: (
      factory: (tui: unknown, theme: unknown, keyboard: unknown, done: (value: unknown) => void) => Component,
    ): Promise<unknown> =>
      new Promise((resolve) => {
        const tui = { requestRender: () => {}, terminal: { columns: 100, rows: 40 } };
        const theme = { fg: (_color: string, text: string) => text, bold: (text: string) => text };
        const entry: OpenedPanel = {
          component: undefined as unknown as Component,
          renderText: (width = 100) => entry.component.render(width).join("\n"),
          keys: (data) => entry.component.handleInput?.(data),
          mouse: (event) => {
            entry.component.render(100);
            entry.component.handleMouse?.(mouseEvent(event));
          },
          click: (x, y) => {
            entry.component.render(100);
            entry.component.handleMouse?.(mouseEvent({ type: "click", x, y, screenX: x, screenY: y }));
          },
          wheel: (x, y, wheelDelta) => {
            entry.component.render(100);
            entry.component.handleMouse?.(mouseEvent({ type: "wheel", x, y, screenX: x, screenY: y, wheelDelta }));
          },
          resolved: false,
        };
        entry.component = factory(tui, theme, null, (value) => {
          entry.resolved = true;
          resolve(value);
        });
        opened.push(entry);
      }),
    notify: (message: string, kind?: string) => {
      notifications.push({ message, kind });
    },
  };
  return { ui, opened, notifications };
}

export function fakeCommandContext(fake: FakeCustomUi, cwd = process.cwd()): unknown {
  return {
    cwd,
    ui: fake.ui,
    modelRegistry: {
      getAvailable: () => [],
      getAll: () => [],
    },
  };
}

/** Let queued microtasks/macrotasks (custom() opens, done() resolution) run. */
export function settle(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}
