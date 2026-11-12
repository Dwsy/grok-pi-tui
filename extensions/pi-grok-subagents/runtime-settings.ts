/**
 * Persisted runtime settings for the subagent pool.
 *
 * One global JSON file at `$GROK_HOME/agents/settings.json` (product-isolated;
 * `productGlobalDir()` resolves `GROK_HOME` / `~/.grok-pi`). Edited through the
 * Pi TUI configuration UI; the runtime reads it on demand so changes apply to
 * the next spawn without restarting the session.
 */

import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { productGlobalDir } from "./definitions.ts";
import { MAX_BACKGROUND_CONCURRENCY } from "./shared.ts";
import { piInput } from "./ui-primitives.ts";

export type SubagentRuntimeSettings = {
  /** Simultaneous background runs across the whole agent tree. */
  backgroundConcurrency?: number;
};

/** Upper bound for the configured concurrency; keeps one runaway value from starving the host. */
export const MAX_BACKGROUND_CONCURRENCY_LIMIT = 256;

export function runtimeSettingsPath(): string {
  return join(productGlobalDir(), "agents", "settings.json");
}

/** Clamp a candidate to a usable concurrency, or `undefined` when unset/invalid. */
export function normalizeBackgroundConcurrency(value: unknown): number | undefined {
  if (value === undefined || value === null || value === "") return undefined;
  const numeric = Number(value);
  if (!Number.isInteger(numeric) || numeric < 1) return undefined;
  return Math.min(numeric, MAX_BACKGROUND_CONCURRENCY_LIMIT);
}

export function loadRuntimeSettings(): SubagentRuntimeSettings {
  const path = runtimeSettingsPath();
  if (!existsSync(path)) return {};
  try {
    const raw: unknown = JSON.parse(readFileSync(path, "utf8"));
    if (typeof raw !== "object" || raw === null) return {};
    const settings = raw as Record<string, unknown>;
    return { backgroundConcurrency: normalizeBackgroundConcurrency(settings.backgroundConcurrency) };
  } catch {
    // A corrupt settings file must not break spawning; fall back to defaults.
    return {};
  }
}

export function saveRuntimeSettings(settings: SubagentRuntimeSettings): void {
  const path = runtimeSettingsPath();
  mkdirSync(join(path, ".."), { recursive: true });
  const temporary = `${path}.tmp`;
  writeFileSync(temporary, `${JSON.stringify(settings, null, 2)}\n`, { encoding: "utf8", mode: 0o600 });
  renameSync(temporary, path);
}

/** Effective background concurrency: configured value or the shipped default. */
export function backgroundConcurrency(): number {
  return loadRuntimeSettings().backgroundConcurrency ?? MAX_BACKGROUND_CONCURRENCY;
}

/** Edit the background concurrency through the Pi TUI input prompt. */
export async function editBackgroundConcurrency(ctx: ExtensionCommandContext): Promise<void> {
  const current = backgroundConcurrency();
  const raw = await piInput(ctx, `Background concurrency (currently ${current})`, {
    initial: String(current),
    hint: `1-${MAX_BACKGROUND_CONCURRENCY_LIMIT} · applies to new spawns · Esc cancels`,
    validate: (value) =>
      normalizeBackgroundConcurrency(value) === undefined
        ? `Enter an integer between 1 and ${MAX_BACKGROUND_CONCURRENCY_LIMIT}.`
        : undefined,
  });
  if (raw === undefined) return;
  const next = normalizeBackgroundConcurrency(raw);
  if (next === undefined) return;
  saveRuntimeSettings({ ...loadRuntimeSettings(), backgroundConcurrency: next });
  ctx.ui.notify(`Background concurrency set to ${next}.`, "info");
}
