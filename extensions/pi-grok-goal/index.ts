/**
 * grok-pi Goal mode (legacy update_goal path).
 *
 * Adapter (GoalHost) is SSOT for Pager GoalUpdated; this extension:
 *  - registers `/goal` slash
 *  - registers `update_goal` tool
 *  - writes process-private control file (PI_GROK_GOAL_CONTROL)
 *  - appends bridge entries so adapter can emit GoalUpdated + continuation
 *
 * Full Grok multi-agent classifier is residual — see docs/issues/adapter/20260722-grok-pi-goal.md
 */
import { randomUUID } from "node:crypto";
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { Type } from "@sinclair/typebox";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

const BRIDGE_TYPE = "pi-grok-goal/v1";
const GOAL_MESSAGE_TYPE = "pi-grok-goal/message/v1";
/** Mirrors codex MAX_THREAD_GOAL_OBJECTIVE_CHARS so both paths reject the same input. */
const MAX_OBJECTIVE_CHARS = 4000;

type GoalEvent =
  | "goal_created"
  | "goal_replaced"
  | "goal_resumed"
  | "goal_paused"
  | "goal_cleared"
  | "goal_completed"
  | "progress";

type GoalMessageOptions = {
  triggerTurn?: boolean;
  deliverAs?: "steer" | "followUp" | "nextTurn";
};

type GoalControl = {
  goalId: string;
  objective: string;
  status: "active" | "user_paused" | "blocked" | "complete" | "cleared";
  phase: "idle" | "planning" | "executing";
  tokenBudget?: number;
  tokenBaseline: number;
  tokensUsed: number;
  createdAt: string;
  pauseMessage?: string;
  lastEvent?: string;
  lastEventDetail?: string;
};

function controlPath(): string | undefined {
  const v = process.env.PI_GROK_GOAL_CONTROL?.trim();
  return v || undefined;
}

function readControl(): GoalControl | undefined {
  const path = controlPath();
  if (!path || !existsSync(path)) return undefined;
  try {
    const raw = JSON.parse(readFileSync(path, "utf8")) as GoalControl;
    if (!raw?.goalId || !raw.objective || !raw.status) return undefined;
    return raw;
  } catch {
    return undefined;
  }
}

function writeControl(control: GoalControl): void {
  const path = controlPath();
  if (!path) return;
  writeFileSync(path, JSON.stringify(control, null, 2), "utf8");
}

/** Codex rejects non-positive budgets; keep one rule for `/goal` and `create_goal`. */
function normalizeBudget(value: unknown): number | undefined {
  if (typeof value !== "number" || !Number.isFinite(value)) return undefined;
  const budget = Math.floor(value);
  return budget > 0 ? budget : undefined;
}

function validateObjective(value: string): string | undefined {
  if (!value) return "goal objective must not be empty";
  if ([...value].length > MAX_OBJECTIVE_CHARS) {
    return `goal objective must be at most ${MAX_OBJECTIVE_CHARS} characters`;
  }
  return undefined;
}

/**
 * Create or replace the goal: control file + bridge entry are the shared state
 * writes. Callers own the model-facing reminder because delivery differs
 * (`/goal` runs outside a turn; `create_goal` runs inside one).
 */
function activateGoal(
  pi: ExtensionAPI,
  objective: string,
  budget?: number,
): { control: GoalControl; replacing: boolean } {
  // Upstream create_goal replaces any existing orchestration; re-set is set, not error.
  const prev = readControl();
  const replacing = !!(prev && prev.status !== "cleared");
  const control: GoalControl = {
    goalId: randomUUID(),
    objective,
    status: "active",
    phase: "executing",
    tokenBudget: budget,
    tokenBaseline: 0,
    tokensUsed: 0,
    createdAt: new Date().toISOString(),
    lastEvent: replacing ? "goal_replaced" : "goal_created",
  };
  writeControl(control);
  emitBridge(pi, control, replacing ? "goal_replaced" : "goal_created");
  return { control, replacing };
}

function parseSetArgs(args: string): { objective: string; budget?: number } {
  const tokens = args.trim().split(/\s+/).filter(Boolean);
  let budget: number | undefined;
  const parts: string[] = [];
  for (let i = 0; i < tokens.length; i++) {
    if (tokens[i] === "--budget" && tokens[i + 1]) {
      budget = normalizeBudget(Number(tokens[i + 1]));
      i++;
      continue;
    }
    parts.push(tokens[i]!);
  }
  return { objective: parts.join(" ").trim(), budget };
}

function goalStateReminder(control: GoalControl, event: GoalEvent, detail?: string): string {
  const cleanDetail = detail?.trim();
  const stateLines = [
    `Event: ${event}`,
    `Objective:\n${control.objective}`,
    `Status: ${control.status}`,
    `Phase: ${control.phase}`,
    control.tokenBudget !== undefined ? `Token budget: ${control.tokenBudget}` : undefined,
    cleanDetail ? `Detail: ${cleanDetail}` : undefined,
  ].filter((line): line is string => Boolean(line));

  const guidance =
    control.status === "active"
      ? [
          "Goal mode is active.",
          "Work until the objective is fully achieved with verifiable evidence.",
          "Prefer concrete checks (tests, builds, file inspection) over claims.",
          'Call create_goal({ objective: "..." }) when the user explicitly asks you to set a goal.',
          'Call update_goal({ completed: true, message: "..." }) ONLY when done.',
          'Call update_goal({ blocked_reason: "..." }) only after repeated failures.',
          'Optional progress: update_goal({ message: "..." }).',
          "Do not stop early. Do not invent success. Continue now.",
        ].join("\n")
      : control.status === "blocked" || control.status === "user_paused"
        ? "Goal mode is paused. Do not continue autonomous goal work until the user resumes or changes the goal."
        : control.status === "complete"
          ? "Goal mode is complete. Do not continue the goal loop unless the user sets or resumes another goal."
          : "Goal mode is cleared. Stop goal-mode behavior for this objective.";

  return `<system-reminder>\n${stateLines.join("\n")}\n\n${guidance}\n</system-reminder>`;
}

function sendGoalMessage(
  pi: ExtensionAPI,
  control: GoalControl,
  event: GoalEvent,
  detail?: string,
  options?: GoalMessageOptions,
): void {
  try {
    pi.sendMessage(
      {
        customType: GOAL_MESSAGE_TYPE,
        content: goalStateReminder(control, event, detail),
        display: false,
        details: {
          version: 1,
          event,
          detail: detail?.trim() || null,
          goalId: control.goalId,
          status: control.status,
          phase: control.phase,
        },
      },
      options,
    );
  } catch {
    // Model-context injection is best-effort; control file + bridge still carry state.
  }
}

function emitBridge(
  pi: ExtensionAPI,
  control: GoalControl,
  event: string,
  detail?: string,
): void {
  // appendEntry keeps traffic out of follow-up/steer queues while streaming.
  try {
    pi.appendEntry(BRIDGE_TYPE, {
      type: "goal_state",
      event,
      detail: detail ?? null,
      control,
    });
  } catch {
    // Bridge is best-effort; control file is still written.
  }
}

export default function piGrokGoal(pi: ExtensionAPI) {
  if (process.env.PI_GROK !== "1") return;
  if (!controlPath()) return;

  pi.registerCommand("goal", {
    description: "Set or manage an autonomous goal (status|pause|resume|clear)",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const trimmed = args.trim();
      const sub = trimmed.split(/\s+/)[0]?.toLowerCase() ?? "";

      if (!trimmed || sub === "help") {
        ctx.ui.notify(
          "Usage: /goal <objective> [--budget N] | status | pause | resume | clear",
          "info",
        );
        return;
      }

      if (sub === "status") {
        const c = readControl();
        if (!c || c.status === "cleared") {
          ctx.ui.notify("No goal is currently set.", "info");
          return;
        }
        ctx.ui.notify(
          `Goal: ${c.objective}\nStatus: ${c.status} | Phase: ${c.phase}`,
          "info",
        );
        return;
      }

      if (sub === "pause") {
        const c = readControl();
        if (!c || c.status !== "active") {
          ctx.ui.notify("No active goal to pause.", "warning");
          return;
        }
        c.status = "user_paused";
        c.phase = "idle";
        c.lastEvent = "goal_paused";
        c.lastEventDetail = "user";
        writeControl(c);
        emitBridge(pi, c, "goal_paused", "user");
        sendGoalMessage(pi, c, "goal_paused", "user", { triggerTurn: false });
        ctx.ui.notify("Goal paused. Use /goal resume to continue.", "info");
        return;
      }

      if (sub === "resume") {
        const c = readControl();
        if (!c || (c.status !== "user_paused" && c.status !== "blocked")) {
          if (c?.status === "active") {
            // Mid-turn resume: queue, don't crash on active stream (upstream GoalSet replaces).
            sendGoalMessage(pi, c, "goal_resumed", "already active", {
              triggerTurn: true,
              deliverAs: "followUp",
            });
            return;
          }
          ctx.ui.notify("No paused goal to resume.", "warning");
          return;
        }
        c.status = "active";
        c.phase = "executing";
        c.pauseMessage = undefined;
        c.lastEvent = "goal_resumed";
        writeControl(c);
        emitBridge(pi, c, "goal_resumed");
        sendGoalMessage(pi, c, "goal_resumed", undefined, {
          triggerTurn: true,
          deliverAs: "followUp",
        });
        return;
      }

      if (sub === "clear") {
        const c = readControl();
        if (!c || c.status === "cleared") {
          ctx.ui.notify("No goal to clear.", "info");
          return;
        }
        c.status = "cleared";
        c.phase = "idle";
        c.lastEvent = "goal_cleared";
        writeControl(c);
        emitBridge(pi, c, "goal_cleared");
        sendGoalMessage(pi, c, "goal_cleared", undefined, { triggerTurn: false });
        ctx.ui.notify("Goal cleared.", "info");
        return;
      }

      const { objective, budget } = parseSetArgs(trimmed);
      if (!objective) {
        ctx.ui.notify("Usage: /goal <objective> [--budget N]", "warning");
        return;
      }

      const invalid = validateObjective(objective);
      if (invalid) {
        ctx.ui.notify(invalid, "warning");
        return;
      }

      const { control, replacing } = activateGoal(pi, objective, budget);
      ctx.ui.notify(
        replacing ? `Goal updated: ${objective}` : `Goal set: ${objective}`,
        "info",
      );
      // deliverAs followUp: mid-turn /goal must queue, never throw "already processing".
      sendGoalMessage(pi, control, replacing ? "goal_replaced" : "goal_created", undefined, {
        triggerTurn: true,
        deliverAs: "followUp",
      });
    },
  });

  pi.registerTool({
    name: "create_goal",
    label: "Create Goal",
    description:
      "Create a persistent goal for this session. Use only when the user explicitly asks you to set a goal; do not infer goals from ordinary tasks. Starts a new active goal when no goal exists, or replaces a completed one. Fails while an unfinished goal exists; use update_goal for status. Set token_budget only when an explicit budget is requested.",
    parameters: Type.Object({
      objective: Type.String({
        description:
          "Required. The concrete objective to pursue. This starts a new active goal, or replaces the current goal.",
      }),
      token_budget: Type.Optional(
        Type.Integer({
          description:
            "Positive token budget for the new goal. Omit unless explicitly requested.",
        }),
      ),
    }),
    async execute(
      _toolCallId: string,
      params: { objective?: string; token_budget?: number },
    ) {
      const objective = params.objective?.trim() ?? "";
      const invalid = validateObjective(objective);
      if (invalid) {
        return {
          content: [{ type: "text", text: `Goal not created: ${invalid}.` }],
          details: { ok: false, reason: "invalid_objective" },
        };
      }

      const prev = readControl();
      if (prev?.status === "active") {
        // Codex refuses to shadow an unfinished goal; /goal remains the replace path.
        return {
          content: [
            {
              type: "text",
              text: `Cannot create a new goal while the current goal is still active: ${prev.objective}\nFinish it with update_goal, or ask the user to replace it with /goal.`,
            },
          ],
          details: { ok: false, reason: "goal_in_progress" },
        };
      }

      const budget = normalizeBudget(params.token_budget);
      if (params.token_budget !== undefined && budget === undefined) {
        return {
          content: [
            { type: "text", text: "Goal not created: goal budgets must be positive." },
          ],
          details: { ok: false, reason: "invalid_budget" },
        };
      }

      const { control, replacing } = activateGoal(pi, objective, budget);
      // Inside a running turn: defer the reminder to turn end instead of starting
      // an extra turn; the adapter's settle-time continuation drives goal work.
      sendGoalMessage(pi, control, replacing ? "goal_replaced" : "goal_created", undefined, {
        triggerTurn: false,
      });
      return {
        content: [
          {
            type: "text",
            text: [
              `Goal created: ${control.objective}`,
              `Status: ${control.status} | Phase: ${control.phase}`,
              control.tokenBudget !== undefined
                ? `Token budget: ${control.tokenBudget}`
                : undefined,
              "Continue working toward it; call update_goal only when done or blocked.",
            ]
              .filter((line): line is string => Boolean(line))
              .join("\n"),
          },
        ],
        details: { ok: true, status: "active", goalId: control.goalId },
      };
    },
  });

  pi.registerTool({
    name: "update_goal",
    label: "Update Goal",
    description:
      "Report goal progress, mark complete, or block. Only use completed=true when fully achieved with evidence.",
    parameters: Type.Object({
      completed: Type.Optional(
        Type.Boolean({
          description:
            "Set true ONLY when the goal is fully achieved. Ends goal mode.",
        }),
      ),
      message: Type.Optional(
        Type.String({
          description: "Short progress or completion summary.",
        }),
      ),
      blocked_reason: Type.Optional(
        Type.String({
          description:
            "Set only when truly stuck after repeated failures. Pauses the goal.",
        }),
      ),
    }),
    async execute(
      _toolCallId: string,
      params: {
        completed?: boolean;
        message?: string;
        blocked_reason?: string;
      },
    ) {
      const c = readControl();
      if (!c || c.status === "cleared" || c.status === "complete") {
        return {
          content: [
            {
              type: "text",
              text: "Goal mode is not active (no /goal run). update_goal has no effect.",
            },
          ],
          details: { ok: false, reason: "inactive" },
        };
      }
      if (c.status === "user_paused" || c.status === "blocked") {
        return {
          content: [
            {
              type: "text",
              text: `Goal is ${c.status}. Use /goal resume before update_goal.`,
            },
          ],
          details: { ok: false, reason: c.status },
        };
      }

      if (params.blocked_reason?.trim()) {
        c.status = "blocked";
        c.phase = "idle";
        c.pauseMessage = params.blocked_reason.trim();
        c.lastEvent = "goal_paused";
        c.lastEventDetail = "blocked";
        writeControl(c);
        emitBridge(pi, c, "goal_paused", "blocked");
        sendGoalMessage(pi, c, "goal_paused", "blocked", {
          triggerTurn: false,
          deliverAs: "nextTurn",
        });
        return {
          content: [
            {
              type: "text",
              text: `Goal blocked: ${c.pauseMessage}`,
            },
          ],
          details: { ok: true, status: "blocked" },
        };
      }

      if (params.completed === true) {
        c.status = "complete";
        c.phase = "idle";
        c.lastEvent = "goal_completed";
        c.lastEventDetail = params.message?.trim() || undefined;
        writeControl(c);
        emitBridge(pi, c, "goal_completed", params.message?.trim());
        sendGoalMessage(pi, c, "goal_completed", params.message?.trim(), {
          triggerTurn: false,
          deliverAs: "nextTurn",
        });
        return {
          content: [
            {
              type: "text",
              text: `Goal completed.${params.message?.trim() ? ` ${params.message.trim()}` : ""}`,
            },
          ],
          details: { ok: true, status: "complete" },
        };
      }

      c.lastEvent = "progress";
      c.lastEventDetail = params.message?.trim() || undefined;
      c.phase = "executing";
      writeControl(c);
      emitBridge(pi, c, "progress", params.message?.trim());
      sendGoalMessage(pi, c, "progress", params.message?.trim(), {
        triggerTurn: false,
        deliverAs: "nextTurn",
      });
      return {
        content: [
          {
            type: "text",
            text: params.message?.trim()
              ? `Progress noted: ${params.message.trim()}`
              : "Progress noted.",
          },
        ],
        details: { ok: true, status: "active" },
      };
    },
  });
}
