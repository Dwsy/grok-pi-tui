import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export const BRIDGE_TYPE = "pi-grok-loop/v1";
export const MIN_INTERVAL_SECS = 60;
export const MAX_TASKS = 50;
export const EXPIRE_MS = 7 * 24 * 60 * 60 * 1000;

export type ScheduledTask = {
  id: string;
  intervalSecs: number;
  prompt: string;
  humanSchedule: string;
  createdAt: string;
  nextFireAt: string;
  fireImmediately: boolean;
  expiresAt: string;
};

export type LoopControl = {
  tasks: ScheduledTask[];
};

export type RuntimeTask = ScheduledTask & {
  timer?: ReturnType<typeof setInterval>;
  firing?: boolean;
};

export const runtime = new Map<string, RuntimeTask>();

export type SchedulerInput = {
  taskId?: string;
  intervalSecs: number;
  prompt: string;
  fireImmediately: boolean;
};
