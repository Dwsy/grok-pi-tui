/**
 * pi-grok-rust-tui-bridge
 *
 * Thin Pi-side bridge that executes component factories and sends pre-rendered
 * frames to the Rust Pager via RPC notifications. Replaces the TS remote-tui's
 * temp-file key transport with direct RPC messaging.
 *
 * Flow:
 *   1. Pi extension calls ctx.ui.custom(factory)
 *   2. This bridge intercepts, executes factory with a stub TUI
 *   3. Calls component.render(width) → text lines
 *   4. Sends lines to Rust via pi/ui/remote_tui notification (op: "frame")
 *   5. Rust forwards key input back via pi/ui/remote_tui/input notification
 *   6. Bridge calls component.handleInput(data), re-renders, sends new frame
 *
 * Supports: SettingsList, Text, Container, Box, Markdown (any Pi TUI component
 * that implements render(width) and handleInput(data)).
 */

import { randomUUID } from "node:crypto";
import { realpathSync } from "node:fs";
import { basename, dirname } from "node:path";
import { pathToFileURL } from "node:url";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import {
  CURSOR_MARKER,
  KeybindingsManager,
  setKeybindings,
  TUI_KEYBINDINGS,
  type Component,
} from "@earendil-works/pi-tui";

// ============================================================================
// Types
// ============================================================================

type ComponentLike = Component & { dispose?(): void };

type BridgeUi = {
  setWidget: (key: string, lines: string[] | undefined, options?: { placement?: string }) => void;
  custom: ((...args: unknown[]) => unknown) & { __rustTuiBridge?: boolean };
  setStatus?: (key: string, text?: string) => void;
  setTitle?: (title: string) => void;
};

type ActiveSession = {
  id: string;
  component: ComponentLike | null;
  focused: Component | null;
  previousComponent: ComponentLike | null;
  closed: boolean;
  width: number;
  close: (result: unknown) => void;
  pushFrame: () => void;
  handleInput: (data: string) => void;
};

// ============================================================================
// Globals
// ============================================================================

let active: ActiveSession | null = null;
const patchedUIs = new WeakSet<object>();

function hostUrl(relativePath: string): string {
  const entryDir = dirname(realpathSync(process.argv[1]!));
  const hostDistDir = basename(entryDir) === "bundle" ? dirname(entryDir) : entryDir;
  return new URL(relativePath, pathToFileURL(hostDistDir).href + "/").href;
}

function resolveViewport(): { width: number; rows: number } {
  const envWidth = Number(process.env.PI_GROK_REMOTE_TUI_WIDTH);
  const columnsEnv = Number(process.env.COLUMNS);
  const stdoutCols = Number(process.stdout?.columns);
  const width = [envWidth, columnsEnv, stdoutCols].find((n) => Number.isFinite(n) && n > 0) ?? 80;
  const rows = Number(process.env.LINES) || Number(process.stdout?.rows) || 24;
  return { width: Math.max(40, Math.floor(width)), rows: Math.max(8, Math.floor(rows)) };
}

// ============================================================================
// RPC notification sender
// ============================================================================

type RpcSender = (method: string, params: Record<string, unknown>) => void;

let sendRpc: RpcSender | null = null;

function notifyRust(op: string, extra: Record<string, unknown> = {}): void {
  sendRpc?.("pi/ui/remote_tui", { op, ...extra });
}

// ============================================================================
// Bridge installation
// ============================================================================

function installBridge(ui: BridgeUi): void {
  if (patchedUIs.has(ui as object) || ui.custom?.__rustTuiBridge) return;

  const original = typeof ui.custom === "function" ? ui.custom.bind(ui) : async () => undefined;

  const bridgeCustom = async (factory: unknown, _options?: unknown) => {
    if (typeof factory !== "function") {
      return original(factory, _options);
    }

    // Tear down previous session
    if (active && !active.closed) {
      active.close(undefined);
    }

    const id = randomUUID();
    const { width, rows } = resolveViewport();

    return new Promise((resolve, reject) => {
      let component: ComponentLike | undefined;
      let closed = false;
      let focused: Component | null = null;
      let previousComponent: ComponentLike | undefined;

      const cleanup = () => {
        notifyRust("close", { id });
        if (active?.id === id) active = null;
        try { component?.dispose?.(); } catch { /* ignore */ }
      };

      const close = (result: unknown) => {
        if (closed) return;
        closed = true;
        cleanup();
        resolve(result);
      };

      const pushFrame = () => {
        if (closed || !component) return;
        try {
          const lines = component
            .render(width)
            .map((line) => String(line).replaceAll(CURSOR_MARKER, ""));
          notifyRust("frame", { id, lines });
        } catch (error) {
          if (closed) return;
          closed = true;
          cleanup();
          reject(error instanceof Error ? error : new Error(String(error)));
        }
      };

      const handleInput = (data: string) => {
        if (closed) return;
        const target = focused ?? component;
        if (target?.handleInput) {
          try {
            target.handleInput(data);
          } catch (error) {
            if (closed) return;
            closed = true;
            cleanup();
            reject(error instanceof Error ? error : new Error(String(error)));
            return;
          }
        }
        pushFrame();
      };

      // Stub TUI for component factories
      const tuiStub = {
        terminal: { columns: width, rows },
        requestRender: () => {
          process.nextTick(() => { if (!closed) pushFrame(); });
        },
        setFocus: (next: Component | null) => { focused = next; },
        showOverlay: (overlay: Component) => {
          if (component && component !== overlay) {
            previousComponent = component;
          }
          component = overlay as ComponentLike;
          focused = overlay;
          notifyRust("overlay_push", { id, title: undefined });
          pushFrame();
          return {
            hide: () => {
              if (closed) return;
              if (previousComponent) {
                component = previousComponent;
                focused = previousComponent;
                previousComponent = undefined;
                notifyRust("overlay_pop", { id });
                pushFrame();
                return;
              }
              focused = component;
              pushFrame();
            },
            show: () => pushFrame(),
            setVisible: (visible: boolean) => {
              if (!visible) tuiStub.hideOverlay();
              else pushFrame();
            },
          };
        },
        hideOverlay: () => {
          if (closed) return;
          if (previousComponent) {
            component = previousComponent;
            focused = previousComponent;
            previousComponent = undefined;
            notifyRust("overlay_pop", { id });
            pushFrame();
          }
        },
        addChild: () => {},
        removeChild: () => {},
      };

      const session: ActiveSession = {
        id,
        component: undefined as unknown as ComponentLike,
        focused: null,
        previousComponent: null,
        closed: false,
        width,
        close,
        pushFrame,
        handleInput,
      };
      active = session;

      // Keybindings manager (components call keybindings.matches(...))
      const keybindings = new KeybindingsManager(TUI_KEYBINDINGS, {});
      setKeybindings(keybindings);

      // Minimal theme stub
      const themeStub = new Proxy({}, {
        get: (_t, prop) => {
          if (prop === "fg") {
            return (color: string, text: string) => {
              const codes: Record<string, string> = {
                accent: "36", success: "32", error: "31",
                warning: "33", dim: "2", muted: "2", border: "90",
              };
              return `\x1b[${codes[color] ?? "0"}m${text}\x1b[0m`;
            };
          }
          if (prop === "bold") return (text: string) => `\x1b[1m${text}\x1b[0m`;
          if (prop === "name") return "rust-tui-bridge";
          return () => "";
        },
      });

      // Notify Rust: open
      notifyRust("open", { id });

      // Execute factory
      try {
        const created = (factory as (tui: unknown, theme: unknown, kb: unknown, done: (r: unknown) => void) => unknown)(
          tuiStub,
          themeStub,
          keybindings,
          close,
        );

        if (closed) {
          try { (created as ComponentLike)?.dispose?.(); } catch { /* ignore */ }
          return;
        }

        component = created as ComponentLike;
        session.component = component;
        focused = component;
        pushFrame();
      } catch (error) {
        if (closed) return;
        closed = true;
        cleanup();
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  };

  (bridgeCustom as typeof bridgeCustom & { __rustTuiBridge?: boolean }).__rustTuiBridge = true;
  ui.custom = bridgeCustom as typeof ui.custom;
  patchedUIs.add(ui as object);
}

// ============================================================================
// Extension entry point
// ============================================================================

export default function (pi: ExtensionAPI): void {
  // Only activate under grok-pi (not native Pi TUI)
  if (process.env.PI_GROK !== "1") return;

  // Set up RPC sender via Pi's event bus
  pi.on("session_start", (_event, ctx) => {
    // Access the RPC notification channel through the UI context
    const ui = ctx.ui as unknown as BridgeUi & {
      _sendNotification?: (method: string, params: unknown) => void;
    };

    // Wire up RPC sender — uses Pi's internal notification mechanism
    if (ui._sendNotification) {
      sendRpc = (method, params) => ui._sendNotification!(method, params);
    } else {
      // Fallback: use setWidget as a signal channel (Rust monitors widget keys)
      sendRpc = (method, params) => {
        if (method === "pi/ui/remote_tui") {
          const p = params as { op: string; id?: string; lines?: string[]; title?: string };
          ui.setWidget("__rust_tui_bridge__", [JSON.stringify(p)], { placement: "aboveEditor" });
          // Clear immediately — Rust reads it synchronously
          ui.setWidget("__rust_tui_bridge__", undefined);
        }
      };
    }

    installBridge(ui);
  });

  // Listen for key input from Rust (forwarded via Pi's notification system)
  pi.on("input", (event, ctx) => {
    // Key input arrives through the remote_tui/input RPC path
    // The Rust side sends it as a Pi notification that triggers this event
    if (!active || active.closed) return;
    if (event.source !== "rpc") return;
    const data = (event as unknown as { data?: string }).data;
    if (typeof data === "string" && data.length > 0) {
      active.handleInput(data);
    }
  });
}
