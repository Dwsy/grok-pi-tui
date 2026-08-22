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
import { join } from "node:path";
import {
  CURSOR_MARKER,
  KeybindingsManager,
  setKeybindings,
  TUI_KEYBINDINGS,
  type Component,
} from "@earendil-works/pi-tui";
import { ensurePiTheme, shouldInstallRemoteHost } from "./env.ts";
import { resolveRemoteTuiLayout, resolveViewport } from "./layout.ts";
import { drainKeys, ensureKeyFile, publishRemoteTuiLayout, writeMeta } from "./transport.ts";
import type { ActiveHost, ComponentLike, RemoteTuiDemoUi, RemoteTuiLayout } from "./shared.ts";
import { WIDGET_KEY } from "./shared.ts";

let active: ActiveHost | null = null;
/** Track which uiContext objects already have our custom() host. */
const patchedUIs = new WeakSet<object>();
export const HOST_MARK = "__piGrokRemoteTuiHost";

type PatchableUi = RemoteTuiDemoUi & {
  custom: ((...args: unknown[]) => unknown) & { [HOST_MARK]?: boolean };
};

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

    return new Promise((resolve, reject) => {
      let component: ComponentLike | undefined;
      let closed = false;
      let focused: Component | null = null;
      let frameWidth = width;
      // Auth select overlays LoginDialog; hide must restore the previous root.
      let previousComponent: ComponentLike | undefined;

      const setLayout = (next: RemoteTuiLayout) => {
        frameWidth = next.width;
        publishRemoteTuiLayout(ui, next);
      };

      const cleanup = () => {
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
        // Clear only the interactive frame. Applied demo surfaces stay so
        // header/footer/status can still be inspected after Esc.
        ui.setWidget(WIDGET_KEY, undefined);
        publishRemoteTuiLayout(ui, undefined);
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
          // Pi components emit this APC sequence only for their in-process
          // terminal renderer to position a hardware cursor. Pager renders the
          // projected frame itself, so forwarding it leaks its `pi:c` payload.
          const lines = component
            .render(frameWidth)
            .map((line) => String(line).replaceAll(CURSOR_MARKER, ""));
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
        // Extension shortcut intercept: check before dispatching to component
        const shortcutIntercept = (globalThis as typeof globalThis & {
          __piGrokShortcutIntercept?: (data: string) => boolean;
        }).__piGrokShortcutIntercept;
        if (shortcutIntercept?.(data)) return;
        const target = focused ?? component;
        if (target?.handleInput) {
          try {
            target.handleInput(data);
          } catch (error) {
            if (closed) return;
            closed = true;
            host.closed = true;
            cleanup();
            reject(error instanceof Error ? error : new Error(String(error)));
            return;
          }
        }
        pushFrame();
      };

      const tuiStub = {
        terminal: { columns: terminalWidth, rows },
        requestRender: () => {
          process.nextTick(() => {
            if (!closed) pushFrame();
          });
        },
        setFocus: (next: Component | null) => {
          focused = next;
        },
        showOverlay: (overlay: Component) => {
          if (component && component !== overlay) {
            previousComponent = component;
          }
          setLayout(resolveRemoteTuiLayout({ overlay: true }, terminalWidth));
          component = overlay as ComponentLike;
          focused = overlay;
          pushFrame();
          return {
            hide: () => {
              if (closed) return;
              if (previousComponent) {
                setLayout(baseLayout);
                component = previousComponent;
                focused = previousComponent;
                previousComponent = undefined;
                pushFrame();
                return;
              }
              // No stacked root (e.g. standalone selector) — keep current frame.
              focused = component ?? null;
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
            focused = previousComponent;
            previousComponent = undefined;
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
        close,
        pushFrame,
        handleInput,
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
        // poll fallback
        const timer = setInterval(() => {
          if (host.closed) {
            clearInterval(timer);
            return;
          }
          drainKeys(host);
        }, 50);
      }

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
          focused = component;
          baseLayout = resolveRemoteTuiLayout(
            _options,
            terminalWidth,
            (component as ComponentLike & { width?: unknown }).width,
          );
          setLayout(baseLayout);
          pushFrame();
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
