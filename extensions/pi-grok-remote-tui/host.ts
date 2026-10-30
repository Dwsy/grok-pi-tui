/**
 * custom() component host — monkey-patches ctx.ui.custom so Pi component
 * factories run in-process while frames project to the Pager via setWidget.
 *
 * Keys arrive through the keyfile transport (transport.ts); layout metadata
 * is published for the Pager overlay renderer (layout.ts).
 */

import { randomUUID } from "node:crypto";
import { existsSync, unlinkSync, watch } from "node:fs";
import { tmpdir } from "node:os";
import { performance } from "node:perf_hooks";
import { join } from "node:path";
import {
  CURSOR_MARKER,
  isKeyRelease,
  KeybindingsManager,
  setKeybindings,
  TUI_KEYBINDINGS,
  type Component,
  type TuiMouseEvent,
  type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import { ensurePiTheme, shouldInstallRemoteHost } from "./env.ts";
import { resolveRemoteTuiLayout, resolveViewport } from "./layout.ts";
import { drainKeys, ensureKeyFile, publishRemoteTuiLayout, writeMeta } from "./transport.ts";
import type { ActiveHost, ComponentLike, RemoteTuiDemoUi, RemoteTuiLayout } from "./shared.ts";
import { SESSION_WIDGET_KEY, WIDGET_KEY } from "./shared.ts";

// Pi does not export its fake-cursor APC markers; Pager needs visible SGR cursors.
const FAKE_CURSOR_START = "\x1b_pi:fc\x07";
const FAKE_CURSOR_END = "\x1b_pi:/fc\x07";

let active: ActiveHost | null = null;
/** Track which uiContext objects already have our custom() host. */
const patchedUIs = new WeakSet<object>();
export const HOST_MARK = "__piGrokRemoteTuiHost";

type PatchableUi = RemoteTuiDemoUi & {
  custom: ((...args: unknown[]) => unknown) & { [HOST_MARK]?: boolean };
};

/** Mirror Pi TUI's focused-component dispatch semantics for remote input. */
export function dispatchComponentInput(target: ComponentLike | null | undefined, data: string): void {
  if (!target?.handleInput) return;
  if (isKeyRelease(data) && !target.wantsKeyRelease) return;
  target.handleInput(data);
}

type MouseDispatchTarget = {
  component: Component;
  originX: number;
  originY: number;
  width: number;
  height: number;
};

type MouseDispatchResult = TuiMouseEventResult & {
  handled: true;
  target: MouseDispatchTarget;
  focusTarget?: Component;
};

/** Mirror Pi TUI's component-level mouse dispatch without owning a terminal. */
function dispatchComponentMouse(component: Component, event: TuiMouseEvent): MouseDispatchResult | undefined {
  const result = component.handleMouse?.(event) as
    | (TuiMouseEventResult & { target?: MouseDispatchTarget; focusTarget?: Component })
    | undefined;
  if (!result) return undefined;
  if (result.target) {
    const forwarded = result as MouseDispatchResult;
    return forwarded.focus && component.handleInput
      ? { ...forwarded, focusTarget: component }
      : forwarded;
  }
  if (!result.handled && !result.capture && !result.focus) return undefined;
  return {
    ...result,
    handled: true,
    ...(result.focus ? { focusTarget: component } : {}),
    target: {
      component,
      originX: event.screenX - event.x,
      originY: event.screenY - event.y,
      width: event.width,
      height: event.height,
    },
  };
}

function retargetMouseEvent(event: TuiMouseEvent, target: MouseDispatchTarget): TuiMouseEvent {
  return {
    ...event,
    x: event.screenX - target.originX,
    y: event.screenY - target.originY,
    width: target.width,
    height: target.height,
  };
}

/** Pi fullscreen TUI's default auto wheel-line acceleration. */
class RemoteWheelScrollAccelerator {
  private lastTime = Number.NEGATIVE_INFINITY;
  private lastDirection = 0;
  private averageGap: number | undefined;
  private carry = 0;
  private readonly accelerate = !(
    process.platform === "darwin" &&
    !process.env.SSH_CONNECTION &&
    !process.env.SSH_CLIENT &&
    !process.env.SSH_TTY
  );

  next(direction: -1 | 1, now: number): number {
    if (!this.accelerate) return 1;
    const gap = now - this.lastTime;
    const sameGesture = direction === this.lastDirection && gap <= 200;
    this.lastTime = now;
    this.lastDirection = direction;
    if (!sameGesture) {
      this.averageGap = undefined;
      this.carry = 0;
      return 1;
    }
    if (gap < 5) return 1;
    this.averageGap = this.averageGap === undefined ? gap : (this.averageGap + gap) / 2;
    const lines = Math.min(6, Math.max(1, 100 / this.averageGap)) + this.carry;
    const whole = Math.floor(lines);
    this.carry = lines - whole;
    return whole;
  }
}

export function installCustomPatch(ui: PatchableUi): void {
  // Pi may rebind uiContext after session_start (noOp → RPC). Patch every new object.
  if (patchedUIs.has(ui as object) || ui.custom?.[HOST_MARK]) {
    return;
  }
  const original = typeof ui.custom === "function" ? ui.custom.bind(ui) : async () => undefined;

  const hostCustom = async (factory: unknown, _options?: unknown) => {
    if (typeof factory !== "function") {
      return original(factory, _options);
    }

    // Tear down previous session if any
    if (active && !active.closed) {
      active.close(undefined);
    }

    const id = randomUUID();
    const { width, rows, terminalWidth, layout } = resolveViewport(_options);
    let baseLayout: RemoteTuiLayout = layout;
    publishRemoteTuiLayout(ui, layout);
    const keysPath = join(tmpdir(), `pi-grok-remote-tui-keys-${id}.jsonl`);
    ensureKeyFile(keysPath);
    writeMeta({ id, keysPath });
    // Claim input before an asynchronous factory can leave a visible/loading UI
    // with the composer still receiving keys. Frames do not own the lifecycle.
    ui.setWidget(SESSION_WIDGET_KEY, [JSON.stringify({ op: "open", id })]);

    return new Promise((resolve, reject) => {
      let component: ComponentLike | undefined;
      const pendingInputs: string[] = [];
      let pollTimer: ReturnType<typeof setInterval> | undefined;
      let closed = false;
      let focused: Component | null = null;
      const setFocus = (next: Component | null) => {
        if (focused && "focused" in focused) (focused as Component & { focused: boolean }).focused = false;
        focused = next;
        if (next && "focused" in next) (next as Component & { focused: boolean }).focused = true;
      };
      let frameWidth = width;
      let mouseCapture: MouseDispatchTarget | undefined;
      let mousePressTarget: MouseDispatchTarget | undefined;
      let mousePressPoint: { x: number; y: number } | undefined;
      let mousePressMoved = false;
      let lastComponentClick:
        | { timestamp: number; count: number; component: Component; x: number; y: number }
        | undefined;
      const wheelScroll = new RemoteWheelScrollAccelerator();
      const clearMouseGesture = () => {
        mouseCapture = undefined;
        mousePressTarget = undefined;
        mousePressPoint = undefined;
        mousePressMoved = false;
      };
      // Auth select overlays LoginDialog; hide must restore the previous root.
      let previousComponent: ComponentLike | undefined;
      let previousFocus: Component | null = null;

      const setLayout = (next: RemoteTuiLayout) => {
        frameWidth = next.width;
        publishRemoteTuiLayout(ui, next);
      };

      const cleanup = () => {
        if (pollTimer) clearInterval(pollTimer);
        clearMouseGesture();
        lastComponentClick = undefined;
        try {
          host.watcher?.close();
        } catch {
          /* ignore */
        }
        try {
          if (existsSync(keysPath)) unlinkSync(keysPath);
        } catch {
          /* ignore */
        }
        writeMeta(null);
        setFocus(null);
        // Clear only the interactive frame. Applied demo surfaces stay so
        // header/footer/status can still be inspected after Esc.
        ui.setWidget(WIDGET_KEY, undefined);
        publishRemoteTuiLayout(ui, undefined);
        ui.setWidget(SESSION_WIDGET_KEY, [JSON.stringify({ op: "close", id })]);
        if (active?.id === id) active = null;
        try {
          component?.dispose?.();
        } catch {
          /* ignore */
        }
      };

      const close = (result: unknown) => {
        if (closed) return;
        closed = true;
        host.closed = true;
        cleanup();
        resolve(result);
      };

      const pushFrame = () => {
        if (closed || !component) return;
        try {
          // Resolve Pi's terminal-only cursor markers before projecting to Pager:
          // discard hardware positioning and preserve fake cursors as reverse video.
          const lines = component
            .render(frameWidth)
            .map((line) =>
              String(line)
                .replaceAll(CURSOR_MARKER, "")
                .replaceAll(FAKE_CURSOR_START, "\x1b[7m")
                .replaceAll(FAKE_CURSOR_END, "\x1b[27m"),
            );
          ui.setWidget(WIDGET_KEY, lines, { placement: "aboveEditor" });
        } catch (error) {
          if (closed) return;
          closed = true;
          host.closed = true;
          cleanup();
          reject(error instanceof Error ? error : new Error(String(error)));
        }
      };

      const handleInput = (data: string) => {
        if (closed) return;
        if (!component) {
          pendingInputs.push(data);
          return;
        }
        // The focused component owns shortcuts until it closes.
        const target = focused ?? component;
        try {
          dispatchComponentInput(target, data);
        } catch (error) {
          if (closed) return;
          closed = true;
          host.closed = true;
          cleanup();
          reject(error instanceof Error ? error : new Error(String(error)));
          return;
        }
        pushFrame();
      };

      const applyMouseDispatchResult = (event: TuiMouseEvent, result: MouseDispatchResult): boolean => {
        const focusTarget = result.focusTarget ?? result.target.component;
        const focusChanged = result.focus === true && focused !== focusTarget;
        if (result.focus) setFocus(focusTarget);
        if (result.capture) mouseCapture = result.target;
        return (
          result.render ??
          (focusChanged ||
            event.type === "press" ||
            event.type === "click" ||
            event.type === "drag" ||
            event.type === "wheel")
        );
      };

      const getComponentClickCount = (target: MouseDispatchTarget, x: number, y: number): number => {
        const now = Date.now();
        const previous = lastComponentClick;
        const count =
          previous &&
          now - previous.timestamp <= 500 &&
          previous.component === target.component &&
          previous.x === x &&
          previous.y === y
            ? (previous.count % 3) + 1
            : 1;
        lastComponentClick = { timestamp: now, count, component: target.component, x, y };
        return count;
      };

      const handleMouse = (incoming: TuiMouseEvent) => {
        if (closed || !component) return;
        let event = incoming;
        if (event.type === "wheel" && event.wheelDelta) {
          const direction: -1 | 1 = event.wheelDelta < 0 ? -1 : 1;
          const lines = wheelScroll.next(direction, performance.now());
          event = {
            ...event,
            wheelDelta: direction * lines * (event.alt ? 5 : 1),
          };
        }

        try {
          let render = false;
          if (mouseCapture || mousePressTarget) {
            const target = mouseCapture ?? mousePressTarget!;
            if (
              mousePressPoint &&
              (event.screenX !== mousePressPoint.x || event.screenY !== mousePressPoint.y)
            ) {
              mousePressMoved = true;
              lastComponentClick = undefined;
            }
            const targetResult = dispatchComponentMouse(
              target.component,
              retargetMouseEvent(event, target),
            );
            if (targetResult) render = applyMouseDispatchResult(event, targetResult);
            if (event.type === "release") {
              if (
                !mousePressMoved &&
                mousePressPoint?.x === event.screenX &&
                mousePressPoint.y === event.screenY
              ) {
                const clickEvent: TuiMouseEvent = {
                  ...event,
                  type: "click",
                  clickCount: getComponentClickCount(target, event.screenX, event.screenY),
                };
                const clickResult = dispatchComponentMouse(
                  target.component,
                  retargetMouseEvent(clickEvent, target),
                );
                if (clickResult) {
                  render = applyMouseDispatchResult(clickEvent, clickResult) || render;
                }
              }
              clearMouseGesture();
            }
          } else {
            const target = focused ?? component;
            const result = dispatchComponentMouse(target, event);
            if (result) {
              render = applyMouseDispatchResult(event, result);
              if (event.type === "press") {
                mousePressTarget = result.target;
                mousePressPoint = { x: event.screenX, y: event.screenY };
                mousePressMoved = false;
              }
            }
          }
          if (render) pushFrame();
        } catch (error) {
          if (closed) return;
          closed = true;
          host.closed = true;
          cleanup();
          reject(error instanceof Error ? error : new Error(String(error)));
        }
      };

      const tuiStub = {
        terminal: { columns: terminalWidth, rows },
        requestRender: () => {
          process.nextTick(() => {
            if (!closed) pushFrame();
          });
        },
        setFocus: (next: Component | null) => {
          setFocus(next);
        },
        showOverlay: (overlay: Component) => {
          if (component && component !== overlay) {
            previousComponent = component;
            previousFocus = focused;
          }
          setLayout(resolveRemoteTuiLayout({ overlay: true }, terminalWidth));
          component = overlay as ComponentLike;
          setFocus(overlay);
          pushFrame();
          return {
            hide: () => {
              if (closed) return;
              if (previousComponent) {
                setLayout(baseLayout);
                component = previousComponent;
                setFocus(previousFocus ?? previousComponent);
                previousComponent = undefined;
                previousFocus = null;
                pushFrame();
                return;
              }
              // No stacked root (e.g. standalone selector) — keep current frame.
              setFocus(component ?? null);
              pushFrame();
            },
            show: () => pushFrame(),
            setVisible: (visible: boolean) => {
              if (!visible) {
                tuiStub.hideOverlay();
              } else {
                pushFrame();
              }
            },
          };
        },
        hideOverlay: () => {
          if (closed) return;
          if (previousComponent) {
            setLayout(baseLayout);
            component = previousComponent;
            setFocus(previousFocus ?? previousComponent);
            previousComponent = undefined;
            previousFocus = null;
            pushFrame();
          }
        },
        addChild: () => {},
        removeChild: () => {},
      };

      const host: ActiveHost = {
        id,
        component: undefined as unknown as ComponentLike,
        closed: false,
        width,
        keysPath,
        watcher: null,
        keyOffset: 0,
        keyBuffer: Buffer.alloc(0),
        close,
        pushFrame,
        handleInput,
        handleMouse,
      };
      active = host;

      // Minimal theme: color helpers return ANSI so frames can render in Grok.
      const themeStub = new Proxy(
        {},
        {
          get: (_t, prop) => {
            if (prop === "fg") {
              return (color: string, text: string) => {
                const codes: Record<string, string> = {
                  accent: "36",
                  success: "32",
                  error: "31",
                  warning: "33",
                  dim: "2",
                  muted: "2",
                  border: "90",
                };
                const code = codes[color] ?? "0";
                return `\x1b[${code}m${text}\x1b[0m`;
              };
            }
            if (prop === "bold") return (text: string) => `\x1b[1m${text}\x1b[0m`;
            if (prop === "name") return "remote-tui-stub";
            return () => "";
          },
        },
      );

      // Real keybindings manager — many Pi components call keybindings.matches(...).
      // Empty {} caused: "this.keybindings.matches is not a function".
      const keybindings = new KeybindingsManager(TUI_KEYBINDINGS, {});
      setKeybindings(keybindings);

      try {
        host.watcher = watch(keysPath, () => drainKeys(host));
      } catch {
        // The periodic drain below also supports platforms without fs.watch.
      }
      // Watch notifications may coalesce a burst before its final write; do not
      // leave its tail unread until the next physical keypress.
      pollTimer = setInterval(() => drainKeys(host), 50);

      // Prefer Pi theme when available (OAuthSelector/LoginDialog touch it).
      // Fall back to themeStub for unit tests / non-Pi argv hosts.
      void ensurePiTheme()
        .catch(() => undefined)
        .then(() =>
          (factory as (tui: unknown, theme: unknown, kb: unknown, done: (r: unknown) => void) => unknown)(
            tuiStub,
            themeStub,
            keybindings,
            close,
          ),
        )
        .then((created) => {
          if (closed) {
            try {
              (created as ComponentLike)?.dispose?.();
            } catch {
              /* ignore */
            }
            return;
          }
          component = created as ComponentLike;
          host.component = component;
          setFocus(focused ?? component);
          baseLayout = resolveRemoteTuiLayout(
            _options,
            terminalWidth,
            (component as ComponentLike & { width?: unknown }).width,
          );
          setLayout(baseLayout);
          pushFrame();
          for (const data of pendingInputs.splice(0)) handleInput(data);
          drainKeys(host);
        })
        .catch((error) => {
          if (closed) return;
          closed = true;
          host.closed = true;
          cleanup();
          reject(error instanceof Error ? error : new Error(String(error)));
        });
    });
  };

  (hostCustom as typeof hostCustom & { [HOST_MARK]?: boolean })[HOST_MARK] = true;
  ui.custom = hostCustom as typeof ui.custom;
  patchedUIs.add(ui as object);
}

/** Other host-injected extensions (auth login/logout) re-bind after RPC ui swaps. */
export function ensureRemoteTuiHost(ui: Parameters<typeof installCustomPatch>[0]): void {
  if (!shouldInstallRemoteHost()) return;
  installCustomPatch(ui);
}

(globalThis as typeof globalThis & {
  __piGrokEnsureRemoteTuiHost?: typeof ensureRemoteTuiHost;
}).__piGrokEnsureRemoteTuiHost = ensureRemoteTuiHost;
