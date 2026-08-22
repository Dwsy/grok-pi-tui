import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

import {
  parseLoopArgs,
  removeTask,
  scheduleInstruction,
  upsertTask,
} from "./scheduler.ts";
import { MIN_INTERVAL_SECS, runtime } from "./shared.ts";

export function registerLoopCommand(pi: ExtensionAPI): void {
  pi.registerCommand("loop", {
    description: "Run a prompt on a recurring interval",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const trimmed = args.trim();
      if (!trimmed || trimmed === "help") {
        ctx.ui.notify(
          "Usage: /loop [interval] <prompt>\nExample: /loop 30m check deploy status\nAlso: /loop list | /loop stop <id>|all",
          "info",
        );
        return;
      }

      const sub = trimmed.split(/\s+/)[0]?.toLowerCase() ?? "";
      if (sub === "list") {
        if (runtime.size === 0) {
          ctx.ui.notify("No scheduled loops.", "info");
          return;
        }
        const lines = [...runtime.values()].map(
          (task) =>
            `${task.id.slice(0, 8)} · ${task.humanSchedule} · next ${task.nextFireAt}\n  ${task.prompt}`,
        );
        ctx.ui.notify(lines.join("\n"), "info");
        return;
      }
      if (sub === "stop" || sub === "cancel") {
        const id = trimmed.slice(sub.length).trim();
        if (!id || id === "all") {
          const ids = [...runtime.keys()];
          for (const taskId of ids) removeTask(pi, taskId, "user");
          ctx.ui.notify(
            ids.length ? `Stopped ${ids.length} loop(s).` : "No loops to stop.",
            "info",
          );
          return;
        }
        // Match by full id or prefix.
        const match =
          runtime.get(id) ??
          [...runtime.values()].find((task) => task.id.startsWith(id));
        if (!match) {
          ctx.ui.notify(`No loop matching ${id}`, "warning");
          return;
        }
        removeTask(pi, match.id, "user");
        ctx.ui.notify(`Stopped loop ${match.id.slice(0, 8)}.`, "info");
        return;
      }

      const { intervalSecs, prompt } = parseLoopArgs(trimmed);
      if (intervalSecs != null && prompt) {
        try {
          const task = upsertTask(pi, {
            intervalSecs,
            prompt,
            fireImmediately: true,
          });
          const raised =
            intervalSecs < MIN_INTERVAL_SECS
              ? ` (raised to ${MIN_INTERVAL_SECS}s minimum)`
              : "";
          ctx.ui.notify(
            `Scheduled ${task.humanSchedule}${raised}. id=${task.id.slice(0, 8)} (expires 7d). Cancel: /loop stop ${task.id.slice(0, 8)}`,
            "info",
          );
        } catch (error) {
          ctx.ui.notify(String(error), "error");
        }
        return;
      }

      // Natural language / no leading interval — model derives via tool.
      pi.sendUserMessage(scheduleInstruction(trimmed));
    },
  });
}
