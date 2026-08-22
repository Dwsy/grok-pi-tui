import { randomUUID } from "node:crypto";

import { loadPersistedTasks, persistRuntime } from "./control.ts";
import { BRIDGE_TYPE, EXPIRE_MS, MAX_TASKS, MIN_INTERVAL_SECS, runtime } from "./shared.ts";
import type { ExtensionAPI, RuntimeTask, ScheduledTask, SchedulerInput } from "./shared.ts";

export function emitBridge(
  pi: ExtensionAPI,
  event: string,
  task: ScheduledTask,
  extra?: Record<string, unknown>,
): void {
  try {
    pi.appendEntry(BRIDGE_TYPE, {
      type: event,
      task,
      ...extra,
    });
  } catch {
    // best-effort
  }
}

export function parseIntervalToken(token: string): number | null {
  const value = token.trim().toLowerCase();
  if (value.length < 2) return null;
  const unit = value.slice(-1);
  const count = Number(value.slice(0, -1));
  if (!Number.isFinite(count) || count <= 0 || !Number.isInteger(count)) return null;
  switch (unit) {
    case "s":
      return count;
    case "m":
      return count * 60;
    case "h":
      return count * 3600;
    case "d":
      return count * 86400;
    default:
      return null;
  }
}

function clampInterval(secs: number): number {
  return Math.max(MIN_INTERVAL_SECS, secs);
}

export function intervalToHuman(secs: number): string {
  if (secs % 86400 === 0) {
    const count = secs / 86400;
    return count === 1 ? "every 1 day" : `every ${count} days`;
  }
  if (secs % 3600 === 0) {
    const count = secs / 3600;
    return count === 1 ? "every 1 hour" : `every ${count} hours`;
  }
  if (secs % 60 === 0) {
    const count = secs / 60;
    return count === 1 ? "every 1 minute" : `every ${count} minutes`;
  }
  return secs === 1 ? "every 1 second" : `every ${secs} seconds`;
}

export function parseLoopArgs(args: string): { intervalSecs: number | null; prompt: string } {
  const trimmed = args.trim();
  const space = trimmed.search(/\s/);
  if (space > 0) {
    const first = trimmed.slice(0, space);
    const rest = trimmed.slice(space).trim();
    const secs = parseIntervalToken(first);
    if (secs != null && rest) {
      return { intervalSecs: secs, prompt: rest };
    }
  }
  return { intervalSecs: null, prompt: trimmed };
}

export function scheduleInstruction(args: string): string {
  return (
    `# /loop -- schedule a recurring prompt\n\n` +
    `Parse the input below into an interval and a prompt, then schedule it with scheduler_create.\n\n` +
    `## Deriving the interval\n` +
    `Read how often to run from the user's request — however they phrase it — and convert it\n` +
    `to a compact \`<number><unit>\` string, where unit is one of \`s\` (seconds), \`m\` (minutes),\n` +
    `\`h\` (hours), or \`d\` (days). The interval may appear at the start or end of the request;\n` +
    `extract it and use the remaining text as the prompt.\n\n` +
    `The minimum interval is 60 seconds; shorter values are raised to 60s, so tell the user if that applies.\n\n` +
    `If the request contains no interval at all, ask the user how often it should run before\n` +
    `scheduling. Do NOT invent or assume a default interval.\n\n` +
    `## Action\n` +
    `1. Call scheduler_create with: interval (the compact string you derived), prompt,\n` +
    `   fire_immediately: true. If the interval is unparseable, the tool\n` +
    `   returns an error — fix the interval string rather than guessing.\n` +
    `2. Confirm: what's scheduled, the cadence, that it auto-expires after 7 days,\n` +
    `   and that they can cancel with scheduler_delete (include the job ID).\n` +
    `3. Do NOT execute the prompt inline. The scheduler will fire it immediately.\n\n` +
    `## Changing an existing loop\n` +
    `Call scheduler_create with its task_id and the fields that change; do not\n` +
    `delete and recreate. If later work changes what a loop should do, update its\n` +
    `prompt the same way.\n\n` +
    `## One-time delayed work\n` +
    `Scheduling is recurring-only. For "do X once in N minutes", run a background\n` +
    `terminal command (\`sleep <secs> && <command>\`); its completion notifies you.\n\n` +
    `## Input\n` +
    `${args}`
  );
}

function firePrompt(task: ScheduledTask): string {
  return (
    `<scheduled-task task_id="${task.id}" schedule="${task.humanSchedule}">\n` +
    `${task.prompt}\n` +
    `</scheduled-task>`
  );
}

function armTimer(pi: ExtensionAPI, task: RuntimeTask): void {
  if (task.timer) {
    clearInterval(task.timer);
    task.timer = undefined;
  }
  const ms = task.intervalSecs * 1000;
  task.timer = setInterval(() => {
    void onFire(pi, task.id);
  }, ms);
  // Prevent keeping process alive solely for timers in some hosts.
  if (typeof task.timer === "object" && task.timer && "unref" in task.timer) {
    try {
      (task.timer as NodeJS.Timeout).unref?.();
    } catch {
      // ignore
    }
  }
}

async function onFire(pi: ExtensionAPI, taskId: string): Promise<void> {
  const task = runtime.get(taskId);
  if (!task) return;
  if (Date.now() >= Date.parse(task.expiresAt)) {
    removeTask(pi, taskId, "expired");
    return;
  }
  if (task.firing) {
    // Skip overlapping fire (upstream skips while previous iteration runs).
    return;
  }
  task.firing = true;
  const next = new Date(Date.now() + task.intervalSecs * 1000).toISOString();
  task.nextFireAt = next;
  persistRuntime();
  emitBridge(pi, "scheduled_task_fired", task, { nextFireAt: next });
  try {
    pi.sendUserMessage(firePrompt(task), { deliverAs: "followUp" });
  } catch {
    // best-effort
  } finally {
    task.firing = false;
  }
}

export function upsertTask(pi: ExtensionAPI, input: SchedulerInput): ScheduledTask {
  const now = Date.now();
  const intervalSecs = clampInterval(input.intervalSecs);
  const humanSchedule = intervalToHuman(intervalSecs);
  const nextFireAt = new Date(
    now + (input.fireImmediately ? 0 : intervalSecs * 1000),
  ).toISOString();

  if (input.taskId) {
    const existing = runtime.get(input.taskId);
    if (!existing) {
      throw new Error(`Unknown task_id: ${input.taskId}`);
    }
    existing.intervalSecs = intervalSecs;
    existing.prompt = input.prompt;
    existing.humanSchedule = humanSchedule;
    existing.nextFireAt = nextFireAt;
    existing.fireImmediately = input.fireImmediately;
    armTimer(pi, existing);
    persistRuntime();
    emitBridge(pi, "scheduled_task_created", existing);
    if (input.fireImmediately) {
      void onFire(pi, existing.id);
    }
    return existing;
  }

  if (runtime.size >= MAX_TASKS) {
    throw new Error(`Maximum ${MAX_TASKS} scheduled tasks`);
  }

  const task: RuntimeTask = {
    id: randomUUID(),
    intervalSecs,
    prompt: input.prompt,
    humanSchedule,
    createdAt: new Date(now).toISOString(),
    nextFireAt,
    fireImmediately: input.fireImmediately,
    expiresAt: new Date(now + EXPIRE_MS).toISOString(),
  };
  runtime.set(task.id, task);
  armTimer(pi, task);
  persistRuntime();
  emitBridge(pi, "scheduled_task_created", task);
  if (input.fireImmediately) {
    void onFire(pi, task.id);
  }
  return task;
}

export function removeTask(pi: ExtensionAPI, taskId: string, reason: string): boolean {
  const task = runtime.get(taskId);
  if (!task) return false;
  if (task.timer) clearInterval(task.timer);
  runtime.delete(taskId);
  persistRuntime();
  emitBridge(pi, "scheduled_task_deleted", task, { reason });
  return true;
}

export function hydrate(pi: ExtensionAPI): void {
  const control = loadPersistedTasks();
  for (const task of control.tasks) {
    if (Date.now() >= Date.parse(task.expiresAt)) continue;
    const runtimeTask: RuntimeTask = { ...task };
    runtime.set(runtimeTask.id, runtimeTask);
    armTimer(pi, runtimeTask);
    emitBridge(pi, "scheduled_task_created", runtimeTask);
  }
}
