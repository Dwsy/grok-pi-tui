import { Type } from "@sinclair/typebox";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdir, readFile, rename, unlink, writeFile } from "node:fs/promises";
import { dirname, isAbsolute, join, resolve } from "node:path";

interface PlanControl {
  active: boolean;
  planFilePath: string;
  syncResponses?: boolean;
}

interface PlanResponse {
  outcome: string;
  message: string;
  planFilePath?: string;
  [key: string]: unknown;
}

const ENTER_TIMEOUT_MS = 10_000;
const EXIT_TIMEOUT_MS = 30 * 60 * 1000;
const POLL_INTERVAL_MS = 25;
const MUTATION_SPAWN_TOOLS = new Set(["spawn_team_agent", "spawn_team", "spawn_subagent"]);

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
    const { active, planFilePath, syncResponses } = value as Record<string, unknown>;
    if (typeof active !== "boolean" || typeof planFilePath !== "string") {
      return undefined;
    }
    if (active && !planFilePath) {
      return undefined;
    }
    return {
      active,
      planFilePath,
      ...(typeof syncResponses === "boolean" ? { syncResponses } : {}),
    };
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

function responseFilePath(controlFilePath: string, toolCallId: string): string {
  const digest = createHash("sha256").update(toolCallId, "utf8").digest("hex");
  return join(dirname(controlFilePath), `plan-resp-${digest}.json`);
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolvePromise, rejectPromise) => {
    if (signal?.aborted) {
      rejectPromise(new Error("aborted"));
      return;
    }
    const onAbort = () => {
      clearTimeout(timer);
      rejectPromise(new Error("aborted"));
    };
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolvePromise();
    }, ms);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

async function waitForPlanResponse(
  controlFilePath: string,
  toolCallId: string,
  timeoutMs: number,
  signal?: AbortSignal,
): Promise<PlanResponse> {
  const path = responseFilePath(controlFilePath, toolCallId);
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (signal?.aborted) throw new Error("aborted");
    if (existsSync(path)) {
      try {
        const parsed: unknown = JSON.parse(await readFile(path, "utf8"));
        if (parsed && typeof parsed === "object") {
          const candidate = parsed as Record<string, unknown>;
          if (typeof candidate.outcome === "string" && typeof candidate.message === "string") {
            await unlink(path).catch(() => undefined);
            return candidate as PlanResponse;
          }
        }
      } catch {
        // Ignore transient partial-read races and retry until deadline.
      }
    }
    await sleep(POLL_INTERVAL_MS, signal);
  }
  return {
    outcome: "error",
    message: `Timed out waiting for host plan-mode response after ${Math.round(timeoutMs / 1000)}s.`,
  };
}

async function atomicWriteFile(path: string, content: string): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  const staged = `${path}.${process.pid}.next`;
  await writeFile(staged, content, "utf8");
  await rename(staged, path);
}

/**
 * Pre-execution write gate and proactive Plan-Mode tools for grok-pi.
 *
 * The adapter writes the process-private control file whenever ACP mode
 * changes or an agent calls `enter_plan_mode` / `exit_plan_mode`. This
 * extension owns no UI; it enforces pre-execution `tool_call` boundaries
 * (blocking bash, non-plan edits, and non-read-only subagent spawns) and
 * blocks tool execution until the adapter completes its state/UI transition.
 */
export default function (pi: {
  on(
    event: "tool_call",
    handler: (
      event: { toolName: string; input: Record<string, unknown> },
      ctx: { cwd: string },
    ) => Promise<unknown>,
  ): void;
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

    if (MUTATION_SPAWN_TOOLS.has(event.toolName)) {
      const capabilityMode =
        typeof event.input?.capability_mode === "string" ? event.input.capability_mode.trim() : "";
      if (capabilityMode === "read-only") return;
      return {
        block: true,
        reason: `Rejected: mutating subagents are disabled in plan mode. Pass capability_mode: "read-only" for read-only exploration, or finish writing ${control.planFilePath} and call exit_plan_mode before spawning implementation agents.`,
      };
    }

    if (event.toolName !== "edit" && event.toolName !== "write") return;
    if (!control.planFilePath) {
      return {
        block: true,
        reason: "Rejected: file edits are not allowed in plan mode because no plan file path is configured.",
      };
    }
    const target = absolutePath(ctx.cwd, event.input?.path);
    if (target === resolve(control.planFilePath)) return;

    return {
      block: true,
      reason: `Rejected: file edits are not allowed in plan mode - the only editable file is the plan file (${control.planFilePath}).`,
    };
  });

  pi.registerTool({
    name: "enter_plan_mode",
    label: "Enter Plan Mode",
    description:
      "Proactively enter read-only Plan Mode to inspect the codebase and write an implementation plan before mutating files or spawning write-enabled subagents. " +
      "Also call this during execution if a `Re-plan if:` condition triggers or a blocked subagent invalidates the current plan. " +
      "Do not call for read-only questions, explanations, code reviews, trivial single-step changes, or when Plan Mode is already active.",
    promptSnippet:
      "Proactively enter Plan Mode before mutating code on complex, ambiguous, or multi-agent tasks, or to re-plan after hitting a blocker.",
    promptGuidelines: [
      "Proactively call enter_plan_mode BEFORE editing files or spawning mutating subagents when a request spans multiple files, involves architectural trade-offs, has ambiguous requirements, or needs Subagents V2 write-scope planning.",
      "During execution, if you hit a Re-plan if: trigger or a blocked subagent invalidates the current plan, call enter_plan_mode to re-plan instead of improvising.",
      "Do not call enter_plan_mode for read-only questions, explanations, code reviews, or trivial single-step edits.",
    ],
    parameters: Type.Object({
      reason: Type.Optional(
        Type.String({
          description: "Brief one-line reason why planning or re-planning is needed before mutating files.",
        }),
      ),
    }),
    async execute(toolCallId: string, _params: { reason?: string }, signal?: AbortSignal) {
      const cPath = controlPath();
      const control = await readControl();
      if (cPath && control?.syncResponses) {
        const resp = await waitForPlanResponse(cPath, toolCallId, ENTER_TIMEOUT_MS, signal);
        return {
          content: [{ type: "text", text: resp.message }],
          details: resp,
        };
      }
      if (control?.active) {
        return {
          content: [
            {
              type: "text",
              text: `Plan mode is already active. Write or update your plan at ${control.planFilePath} and call exit_plan_mode when ready.`,
            },
          ],
          details: { outcome: "entered", alreadyActive: true, planFilePath: control.planFilePath },
        };
      }
      if (cPath && control) {
        const updated: PlanControl = { ...control, active: true };
        if (updated.planFilePath && !existsSync(updated.planFilePath)) {
          await mkdir(dirname(updated.planFilePath), { recursive: true });
          await writeFile(updated.planFilePath, "", "utf8");
        }
        await atomicWriteFile(cPath, JSON.stringify(updated));
        return {
          content: [
            {
              type: "text",
              text: `Entered plan mode. Write your structured plan to ${updated.planFilePath} and call exit_plan_mode proactively when ready.`,
            },
          ],
          details: { outcome: "entered", alreadyActive: false, planFilePath: updated.planFilePath },
        };
      }
      return {
        content: [
          {
            type: "text",
            text: "Plan mode host control is unavailable in this environment; continue planning directly.",
          },
        ],
        details: { outcome: "error" },
      };
    },
  });

  // The adapter observes this tool's normal lifecycle, opens Grok's native
  // plan-approval surface using x.ai/exit_plan_mode, updates control.json, and
  // writes the per-call response file so the turn observes the exact verdict.
  pi.registerTool({
    name: "exit_plan_mode",
    label: "Exit Plan Mode",
    description:
      "Submit the current plan for user approval and exit Plan Mode. " +
      "Prefer writing the plan to the session plan file via write/edit first; optionally pass `plan` to persist it before approval. " +
      "Call proactively in the same turn as soon as the plan is ready instead of printing a plan in chat prose and leaving Plan Mode active.",
    promptSnippet:
      "Submit the completed plan for user approval and exit Plan Mode; call proactively as soon as the plan is ready.",
    promptGuidelines: [
      "Never end a planning turn by pasting a plan in chat text while leaving Plan Mode active. Once the plan is written (Goal, Ordered Steps with files/symbols, Write Scopes if delegating, Verification commands, and Re-plan if: triggers) and no blocking question remains, call exit_plan_mode immediately in the same turn.",
      "Follow the exit_plan_mode verdict strictly: on approved, start implementing right away without calling exit_plan_mode again; on abandoned, stop without editing files and wait for the user; on cancelled, stay in Plan Mode and revise the plan per feedback.",
    ],
    parameters: Type.Object({
      plan: Type.Optional(
        Type.String({
          description:
            "Optional complete Markdown plan. Prefer writing to the session plan file first via write/edit; if supplied here, the host persists it before opening plan approval.",
        }),
      ),
    }),
    async execute(toolCallId: string, params: { plan?: string }, signal?: AbortSignal) {
      const cPath = controlPath();
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
          details: { outcome: "inactive" },
        };
      }
      if (cPath && control.syncResponses) {
        const resp = await waitForPlanResponse(cPath, toolCallId, EXIT_TIMEOUT_MS, signal);
        return {
          content: [{ type: "text", text: resp.message }],
          details: { ...resp, planFilePath: resp.planFilePath ?? control.planFilePath },
        };
      }
      if (typeof params?.plan === "string" && params.plan.trim() && control.planFilePath) {
        const body = params.plan.endsWith("\n") ? params.plan : `${params.plan}\n`;
        await atomicWriteFile(control.planFilePath, body);
      }
      return {
        content: [{ type: "text", text: "Plan submitted for user approval." }],
        details: { planFilePath: control.planFilePath },
      };
    },
  });
}
