import { Type } from "@sinclair/typebox";

import { parseIntervalToken, removeTask, upsertTask } from "./scheduler.ts";
import { runtime } from "./shared.ts";
import type { ExtensionAPI } from "./shared.ts";

export function registerSchedulerTools(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "scheduler_create",
    label: "Scheduler Create",
    description:
      "Create a scheduled task that runs a prompt on a recurring interval, or update an existing one in place. Interval: 5m/2h/1d/60s (min 60s). fire_immediately defaults false for the tool; /loop uses true.",
    parameters: Type.Object({
      task_id: Type.Optional(
        Type.String({
          description: "Existing task id to update in place.",
        }),
      ),
      interval: Type.Optional(
        Type.String({
          description: 'Interval e.g. "5m", "2h", "1d", "60s". Required to create.',
        }),
      ),
      prompt: Type.Optional(
        Type.String({
          description: "Prompt text for each fire. Required to create.",
        }),
      ),
      fire_immediately: Type.Optional(
        Type.Boolean({
          description:
            "Fire once on create/update (true) or wait for first interval (false). Default false.",
        }),
      ),
    }),
    async execute(
      _toolCallId: string,
      params: {
        task_id?: string;
        interval?: string;
        prompt?: string;
        fire_immediately?: boolean;
      },
    ) {
      try {
        if (params.task_id) {
          const existing = runtime.get(params.task_id);
          if (!existing) {
            return {
              content: [{ type: "text", text: `Unknown task_id: ${params.task_id}` }],
              details: { ok: false, id: params.task_id, humanSchedule: "", updated: false },
            };
          }
          const secs = params.interval
            ? parseIntervalToken(params.interval)
            : existing.intervalSecs;
          if (secs == null) {
            return {
              content: [{ type: "text", text: `Invalid interval: ${params.interval}` }],
              details: { ok: false, id: params.task_id, humanSchedule: "", updated: false },
            };
          }
          const task = upsertTask(pi, {
            taskId: params.task_id,
            intervalSecs: secs,
            prompt: params.prompt?.trim() || existing.prompt,
            fireImmediately: params.fire_immediately === true,
          });
          return {
            content: [
              {
                type: "text",
                text: `Updated task ${task.id} (${task.humanSchedule}).`,
              },
            ],
            details: {
              ok: true,
              id: task.id,
              humanSchedule: task.humanSchedule,
              updated: true,
            },
          };
        }

        const interval = params.interval?.trim();
        const prompt = params.prompt?.trim();
        if (!interval || !prompt) {
          return {
            content: [
              {
                type: "text",
                text: "interval and prompt are required to create a task.",
              },
            ],
            details: { ok: false, id: "", humanSchedule: "", updated: false },
          };
        }
        const secs = parseIntervalToken(interval);
        if (secs == null) {
          return {
            content: [{ type: "text", text: `Invalid interval: ${interval}` }],
            details: { ok: false, id: "", humanSchedule: "", updated: false },
          };
        }
        const task = upsertTask(pi, {
          intervalSecs: secs,
          prompt,
          fireImmediately: params.fire_immediately === true,
        });
        return {
          content: [
            {
              type: "text",
              text: `Scheduled ${task.humanSchedule}. id=${task.id}. Auto-expires after 7 days. Cancel with scheduler_delete.`,
            },
          ],
          details: {
            ok: true,
            id: task.id,
            humanSchedule: task.humanSchedule,
            updated: false,
          },
        };
      } catch (error) {
        return {
          content: [{ type: "text", text: String(error) }],
          details: { ok: false, id: "", humanSchedule: "", updated: false },
        };
      }
    },
  });

  pi.registerTool({
    name: "scheduler_delete",
    label: "Scheduler Delete",
    description: "Cancel a scheduled task by task_id (from scheduler_create).",
    parameters: Type.Object({
      task_id: Type.String({ description: "Task ID to cancel" }),
    }),
    async execute(_toolCallId: string, params: { task_id: string }) {
      const ok = removeTask(pi, params.task_id, "delete");
      return {
        content: [
          {
            type: "text",
            text: ok
              ? `Deleted scheduled task ${params.task_id}`
              : `No task ${params.task_id}`,
          },
        ],
        details: { ok, task_id: params.task_id },
      };
    },
  });

  pi.registerTool({
    name: "scheduler_list",
    label: "Scheduler List",
    description: "List active scheduled tasks for this session.",
    parameters: Type.Object({}),
    async execute() {
      const tasks = [...runtime.values()].map((task) => ({
        id: task.id,
        human_schedule: task.humanSchedule,
        prompt: task.prompt,
        next_fire_at: task.nextFireAt,
      }));
      return {
        content: [
          {
            type: "text",
            text:
              tasks.length === 0
                ? "No scheduled tasks."
                : tasks
                    .map(
                      (task) =>
                        `${task.id} · ${task.human_schedule} · next ${task.next_fire_at}\n  ${task.prompt}`,
                    )
                    .join("\n"),
          },
        ],
        details: { tasks },
      };
    },
  });
}
