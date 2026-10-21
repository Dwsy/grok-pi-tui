import { Type } from "@sinclair/typebox";
import { readFile } from "node:fs/promises";
import { isAbsolute, resolve } from "node:path";

interface PlanControl {
  active: boolean;
  planFilePath: string;
}

function controlPath(): string | undefined {
  const value = process.env.PI_GROK_PLAN_CONTROL?.trim();
  return value || undefined;
}

async function readControl(): Promise<PlanControl | undefined> {
  const path = controlPath();
  if (!path) return undefined;
  try {
    const value: unknown = JSON.parse(await readFile(path, "utf8"));
    if (!value || typeof value !== "object") return undefined;
    const { active, planFilePath } = value as Record<string, unknown>;
    if (typeof active !== "boolean" || typeof planFilePath !== "string" || !planFilePath) {
      return undefined;
    }
    return { active, planFilePath };
  } catch {
    // The adapter is the authority. A missing/invalid transient control file
    // must not invent plan-mode restrictions or break the agent loop.
    return undefined;
  }
}

function absolutePath(cwd: string, candidate: unknown): string | undefined {
  if (typeof candidate !== "string" || !candidate.trim()) return undefined;
  return isAbsolute(candidate) ? resolve(candidate) : resolve(cwd, candidate);
}

/**
 * Pre-execution write gate for grok-pi Plan mode.
 *
 * The adapter writes the process-private control file whenever ACP mode
 * changes. This extension owns no UI and no plan state; it only applies the
 * native Pi `tool_call` interception point before a mutation can happen.
 */
export default function (pi: {
  on(event: "tool_call", handler: (event: { toolName: string; input: Record<string, unknown> }, ctx: { cwd: string }) => Promise<unknown>): void;
  registerTool(definition: Record<string, unknown>): void;
}) {
  pi.on("tool_call", async (event, ctx) => {
    const control = await readControl();
    if (!control?.active) return;

    if (event.toolName === "bash") {
      return {
        block: true,
        reason: `Rejected: bash is disabled in plan mode. The only writable file is ${control.planFilePath}.`,
      };
    }

    if (event.toolName !== "edit" && event.toolName !== "write") return;
    const target = absolutePath(ctx.cwd, event.input.path);
    if (target === resolve(control.planFilePath)) return;

    return {
      block: true,
      reason: `Rejected: file edits are not allowed in plan mode - the only editable file is the plan file (${control.planFilePath}).`,
    };
  });

  // The adapter observes this tool's normal lifecycle and opens Grok's native
  // plan-approval surface using x.ai/exit_plan_mode. It intentionally carries
  // no decision logic or UI in the Pi process.
  pi.registerTool({
    name: "exit_plan_mode",
    label: "Exit Plan Mode",
    description: "Submit the current plan for user approval before implementation.",
    parameters: Type.Object({}),
    async execute() {
      const control = await readControl();
      if (!control?.active) {
        // The adapter approves exit_plan_mode mid-turn and immediately flips
        // the control file to inactive. A model retry racing that transition
        // must observe idempotent success, not a thrown error, or it loops
        // under the stale "Plan mode is active" reminder.
        return {
          content: [
            {
              type: "text",
              text: "Plan mode is not active. Nothing to submit; continue in the current mode.",
            },
          ],
          details: {},
        };
      }
      return {
        content: [{ type: "text", text: "Plan submitted for user approval." }],
        details: {},
      };
    },
  });
}
