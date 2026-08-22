import { watch } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionContext } from "@earendil-works/pi-coding-agent";

export interface ShortcutEntry {
  key: string;
  description: string;
  extensionPath: string;
  enabled: boolean;
  remappedTo?: string;
}

export interface ShortcutManagerConfig {
  version: 1;
  shortcuts: Record<string, ShortcutEntry>;
  globalEnabled: boolean;
}

export interface RegisteredShortcut {
  key: string;
  description: string;
  extensionPath: string;
  handler: (ctx: ExtensionContext) => Promise<void> | void;
}

export interface ShortcutDiagnostic {
  key: string;
  extensionPath: string;
  description: string;
  conflictType: "duplicate" | "none";
  conflictWith?: string;
  enabled: boolean;
  remappedTo?: string;
}

export type WidgetUi = {
  setWidget?: (key: string, content: string[] | undefined) => void;
};

export type ExtensionRunnerLike = {
  setUIContext: (uiContext: unknown, mode?: string) => void;
  getShortcuts: (resolvedKeybindings: unknown) => Map<string, {
    shortcut: string;
    description?: string;
    handler: (ctx: ExtensionContext) => Promise<void> | void;
    extensionPath: string;
  }>;
};

export type ExtensionRunnerConstructor = {
  prototype: ExtensionRunnerLike & { __piGrokShortcutManagerPatched?: boolean };
};

export const CONFIG_DIR = join(homedir(), ".pi");
export const CONFIG_PATH = join(CONFIG_DIR, "shortcut-manager.json");
export const DISPATCH_META_PATH = join(tmpdir(), "pi-grok-shortcut-dispatch-active.json");
export const SYNC_WIDGET_KEY = "__pi_extension_shortcuts__";

export const state = {
  configCache: null as ShortcutManagerConfig | null,
  shortcutRegistry: new Map<string, RegisteredShortcut>(),
  latestCtx: null as ExtensionContext | null,
  runnerInstance: null as unknown,
  dispatchKeysPath: null as string | null,
  dispatchOffset: 0,
  dispatchWatcher: null as ReturnType<typeof watch> | null,
  dispatchPollTimer: null as ReturnType<typeof setInterval> | null,
};
