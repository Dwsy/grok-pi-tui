import { realpathSync } from "node:fs";
import { dirname } from "node:path";
import { pathToFileURL } from "node:url";

import { loadConfig } from "./config.ts";
import { dispatchShortcut } from "./dispatch.ts";
import { SYNC_WIDGET_KEY, state } from "./shared.ts";
import type {
  ExtensionRunnerConstructor,
  ExtensionRunnerLike,
  ShortcutDiagnostic,
  WidgetUi,
} from "./shared.ts";

function hostUrl(relativePath: string): string {
  const hostDistDir = dirname(realpathSync(process.argv[1]!));
  return new URL(relativePath, pathToFileURL(`${hostDistDir}/`)).href;
}

export async function installRunnerCapture(): Promise<void> {
  try {
    const module = (await import(hostUrl("core/extensions/runner.js"))) as {
      ExtensionRunner?: ExtensionRunnerConstructor;
    };
    const prototype = module.ExtensionRunner?.prototype;
    if (!prototype) return;
    if (prototype.__piGrokShortcutManagerPatched) return;

    const original = prototype.setUIContext;
    prototype.setUIContext = function setUIContext(this: ExtensionRunnerLike, uiContext: unknown, mode?: string): void {
      original.call(this, uiContext, mode);
      // Capture runner; always re-read shortcuts (extensions can register late).
      state.runnerInstance = this;
      refreshRegistry();
      const ui = uiContext as WidgetUi | undefined;
      publishRegistryToHost(ui);
    };
    prototype.__piGrokShortcutManagerPatched = true;
  } catch {
    // Runner not available
  }
}

export function refreshRegistry(): void {
  if (!state.runnerInstance) return;
  const runner = state.runnerInstance as ExtensionRunnerLike;
  try {
    const shortcuts = runner.getShortcuts({});
    state.shortcutRegistry.clear();
    for (const [key, shortcut] of shortcuts) {
      const normalizedKey = key.toLowerCase();
      state.shortcutRegistry.set(normalizedKey, {
        key,
        description: shortcut.description ?? shortcut.extensionPath,
        extensionPath: shortcut.extensionPath,
        handler: shortcut.handler,
      });
    }
  } catch {
    // getShortcuts may fail if keybindings config is not ready
  }
}

/**
 * Project the in-process registry to Rust via Pi RPC setWidget.
 * Native `/pi-shortcut-manager` reads this into ExtensionShortcutRegistry.
 * Does not touch remote-tui.
 */
export function publishRegistryToHost(ui?: WidgetUi): void {
  if (!ui?.setWidget) return;
  const config = loadConfig();
  const payload = Array.from(state.shortcutRegistry.values()).map((s) => {
    const pref = config.shortcuts[s.key.toLowerCase()];
    return {
      key: s.key,
      description: s.description,
      extension: shortExtName(s.extensionPath),
      enabled: pref ? pref.enabled : true,
      remappedTo: pref?.remappedTo,
    };
  });
  try {
    // One JSON line — Pager intercepts the widget key and never paints it.
    ui.setWidget(SYNC_WIDGET_KEY, [JSON.stringify({ shortcuts: payload })]);
    // Clear so sticky widget surface stays empty (payload already delivered).
    ui.setWidget(SYNC_WIDGET_KEY, undefined);
  } catch {
    /* host may reject */
  }
}

export function installGlobalIntercept(): void {
  const g = globalThis as typeof globalThis & {
    __piGrokShortcutIntercept?: (data: string) => boolean;
  };
  g.__piGrokShortcutIntercept = dispatchShortcut;
}

export function buildDiagnostics(): ShortcutDiagnostic[] {
  const config = loadConfig();
  const diagnostics: ShortcutDiagnostic[] = [];
  const seenKeys = new Map<string, string>();

  for (const [key, shortcut] of state.shortcutRegistry) {
    const normalizedKey = key.toLowerCase();
    let conflictType: ShortcutDiagnostic["conflictType"] = "none";
    let conflictWith: string | undefined;

    if (seenKeys.has(normalizedKey)) {
      conflictType = "duplicate";
      conflictWith = seenKeys.get(normalizedKey);
    }
    seenKeys.set(normalizedKey, shortcut.extensionPath);

    const entry = config.shortcuts[normalizedKey];
    diagnostics.push({
      key,
      extensionPath: shortcut.extensionPath,
      description: shortcut.description,
      conflictType,
      conflictWith,
      enabled: entry ? entry.enabled : true,
      remappedTo: entry?.remappedTo,
    });
  }

  return diagnostics;
}

export function shortExtName(extPath: string): string {
  // "pi-language-tutor" from "/Users/x/.pi/agent/npm/node_modules/pi-language-tutor/src/index.ts"
  const parts = extPath.split("/");
  const nmIdx = parts.lastIndexOf("node_modules");
  if (nmIdx >= 0 && nmIdx + 1 < parts.length) return parts[nmIdx + 1]!;
  // Fallback: last directory or filename
  return parts[parts.length - 2] ?? parts[parts.length - 1] ?? extPath;
}

export function formatKeyDisplay(key: string): string {
  return key.toUpperCase().replace(/\+/g, "+");
}
