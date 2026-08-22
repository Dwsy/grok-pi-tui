import { existsSync, readFileSync, writeFileSync } from "node:fs";

import { runtime } from "./shared.ts";
import type { LoopControl } from "./shared.ts";

export function controlPath(): string | undefined {
  const value = process.env.PI_GROK_LOOP_CONTROL?.trim();
  return value || undefined;
}

function readControl(): LoopControl {
  const path = controlPath();
  if (!path || !existsSync(path)) return { tasks: [] };
  try {
    const raw = JSON.parse(readFileSync(path, "utf8")) as LoopControl;
    if (!Array.isArray(raw?.tasks)) return { tasks: [] };
    return { tasks: raw.tasks };
  } catch {
    return { tasks: [] };
  }
}

function writeControl(control: LoopControl): void {
  const path = controlPath();
  if (!path) return;
  writeFileSync(path, JSON.stringify(control, null, 2), "utf8");
}

export function persistRuntime(): void {
  const tasks = [...runtime.values()].map(
    ({ timer: _timer, firing: _firing, ...task }) => task,
  );
  writeControl({ tasks });
}

export function loadPersistedTasks(): LoopControl {
  return readControl();
}
