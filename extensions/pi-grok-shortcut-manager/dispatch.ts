import { existsSync, readFileSync, watch, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { matchesKey, type KeyId } from "@earendil-works/pi-tui";

import { getEffectiveKey, isShortcutEnabled, loadConfig } from "./config.ts";
import { DISPATCH_META_PATH, state } from "./shared.ts";

export function runHandler(key: string): boolean {
  if (!state.latestCtx) {
    console.error(`[shortcut-manager] no ctx for '${key}' (wait for session_start)`);
    return false;
  }
  const shortcut = state.shortcutRegistry.get(key.toLowerCase());
  if (!shortcut) {
    console.error(`[shortcut-manager] unknown key '${key}' registry=${state.shortcutRegistry.size}`);
    return false;
  }
  if (!isShortcutEnabled(key)) return false;
  try {
    // Prefer a fresh notify so user sees the fire even if extension UI is limited in RPC.
    try {
      state.latestCtx.ui?.notify?.(`Running shortcut ${key}`, "info");
    } catch {
      /* ignore */
    }
    Promise.resolve(shortcut.handler(state.latestCtx)).catch((err) => {
      console.error(`[shortcut-manager] Handler error for '${key}':`, err);
      try {
        state.latestCtx?.ui?.notify?.(`Shortcut ${key} failed: ${err}`, "error");
      } catch {
        /* ignore */
      }
    });
  } catch (err) {
    console.error(`[shortcut-manager] Sync handler error for '${key}':`, err);
    return false;
  }
  return true;
}

/** Dispatch from remote-tui key sequences (matchesKey on terminal data). */
export function dispatchShortcut(data: string): boolean {
  if (!state.latestCtx) return false;
  const config = loadConfig();
  if (!config.globalEnabled) return false;

  for (const [key] of state.shortcutRegistry) {
    if (!isShortcutEnabled(key)) continue;
    const effectiveKey = getEffectiveKey(key);
    if (matchesKey(data, effectiveKey as KeyId)) {
      return runHandler(key);
    }
  }
  return false;
}

/** Dispatch from Rust Pager (already matched KeyId, e.g. "alt+t"). */
export function dispatchByKeyId(key: string): boolean {
  if (!state.latestCtx) return false;
  const config = loadConfig();
  if (!config.globalEnabled) return false;
  return runHandler(key);
}

function resolveDispatchKeysPath(): string {
  const fromEnv = process.env.PI_GROK_SHORTCUT_KEYS?.trim();
  if (fromEnv) return fromEnv;
  // Legacy: TS-created pid keyfile + global meta (racy multi-process).
  return join(tmpdir(), `pi-grok-shortcut-keys-${process.pid}.jsonl`);
}

export function ensureDispatchChannel(): void {
  const keysPath = resolveDispatchKeysPath();
  if (state.dispatchKeysPath === keysPath && existsSync(keysPath)) {
    // Channel already armed for this path.
    if (!state.dispatchPollTimer) startDispatchPoll();
    return;
  }
  try {
    if (!existsSync(keysPath)) writeFileSync(keysPath, "");
    // Keep legacy meta for older adapters that still read it.
    if (!process.env.PI_GROK_SHORTCUT_KEYS) {
      writeFileSync(
        DISPATCH_META_PATH,
        JSON.stringify({ keysPath, pid: process.pid }),
        "utf8",
      );
    }
    state.dispatchKeysPath = keysPath;
    state.dispatchOffset = 0;
    state.dispatchWatcher?.close();
    try {
      state.dispatchWatcher = watch(keysPath, () => drainDispatchKeys());
    } catch {
      state.dispatchWatcher = null;
    }
    startDispatchPoll();
  } catch (err) {
    console.error("[shortcut-manager] dispatch channel setup failed:", err);
  }
}

function startDispatchPoll(): void {
  if (state.dispatchPollTimer) return;
  // fs.watch is unreliable on some platforms / network tmp; poll as backup.
  state.dispatchPollTimer = setInterval(() => drainDispatchKeys(), 50);
  // Don't keep the process alive solely for this timer.
  state.dispatchPollTimer.unref?.();
}

function drainDispatchKeys(): void {
  if (!state.dispatchKeysPath || !existsSync(state.dispatchKeysPath)) return;
  try {
    const raw = readFileSync(state.dispatchKeysPath, "utf8");
    if (raw.length <= state.dispatchOffset) return;
    const chunk = raw.slice(state.dispatchOffset);
    state.dispatchOffset = raw.length;
    for (const line of chunk.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        const event = JSON.parse(trimmed) as { op?: string; key?: string };
        if (event.op === "dispatch" && typeof event.key === "string") {
          dispatchByKeyId(event.key);
        }
      } catch {
        /* ignore bad line */
      }
    }
  } catch {
    /* ignore */
  }
}
