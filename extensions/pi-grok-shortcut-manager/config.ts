import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";

import { CONFIG_DIR, CONFIG_PATH, state } from "./shared.ts";
import type { ShortcutManagerConfig } from "./shared.ts";

export function loadConfig(): ShortcutManagerConfig {
  if (state.configCache) return state.configCache;
  try {
    if (existsSync(CONFIG_PATH)) {
      const raw = readFileSync(CONFIG_PATH, "utf8");
      state.configCache = JSON.parse(raw) as ShortcutManagerConfig;
      return state.configCache;
    }
  } catch { /* ignore corrupt config */ }
  state.configCache = { version: 1, shortcuts: {}, globalEnabled: true };
  return state.configCache;
}

export function saveConfig(config: ShortcutManagerConfig): void {
  state.configCache = config;
  try {
    if (!existsSync(CONFIG_DIR)) mkdirSync(CONFIG_DIR, { recursive: true });
    writeFileSync(CONFIG_PATH, JSON.stringify(config, null, 2), "utf8");
  } catch { /* best effort */ }
}

export function isShortcutEnabled(key: string): boolean {
  const config = loadConfig();
  if (!config.globalEnabled) return false;
  const entry = config.shortcuts[key.toLowerCase()];
  if (entry && !entry.enabled) return false;
  return true;
}

export function getEffectiveKey(key: string): string {
  const config = loadConfig();
  const entry = config.shortcuts[key.toLowerCase()];
  return entry?.remappedTo ?? key;
}
