/**
 * grok-pi Goal mode (legacy goal tool path).
 *
 * Adapter (GoalHost) is SSOT for Pager GoalUpdated; this extension:
 *  - registers `/goal` slash
 *  - registers one `goal` tool with create/get/update/resume actions
 *  - projects session-bound state into PI_GROK_GOAL_CONTROL for the adapter
 *  - persists snapshots as Pi SessionManager custom entries (durable SSOT)
 *
 * Full Grok multi-agent classifier is residual — see docs/issues/adapter/20260722-grok-pi-goal.md
 */
import { randomUUID } from "node:crypto";
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { Type } from "@sinclair/typebox";
import type { ExtensionAPI, ExtensionCommandContext, ExtensionContext } from "@earendil-works/pi-coding-agent";

const BRIDGE_TYPE = "pi-grok-goal/v1";
const GOAL_MESSAGE_TYPE = "pi-grok-goal/message/v1";
/**
 * Hidden command the adapter invokes after writing an adapter-originated
 * transition (continuation loop guard) so it also lands in Pi's session log.
 */
const PERSIST_COMMAND = "__pi_grok_goal_persist";
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
  deliverAs?: "steer" | "followUp";
};

type GoalControl = {
  sessionId: string;
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

function readControl(sessionId: string): GoalControl | undefined {
  const path = controlPath();
  if (!path || !existsSync(path)) return undefined;
  try {
    const raw = JSON.parse(readFileSync(path, "utf8")) as GoalControl;
    if (!raw || raw.sessionId !== sessionId || !raw.status) return undefined;
    if (raw.status !== "cleared" && (!raw.goalId || !raw.objective)) return undefined;
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

/** Session-scoped tombstone: clears the Pager without erasing the Pi journal. */
function noGoal(sessionId: string): GoalControl {
  return { sessionId, goalId: "", objective: "", status: "cleared", phase: "idle",
    tokenBaseline: 0, tokensUsed: 0, createdAt: "" };
}

function isUnfinished(goal: GoalControl | undefined): goal is GoalControl {
  return !!goal && (
    goal.status === "active" || goal.status === "user_paused" || goal.status === "blocked"
  );
}

/** Rebuild state from Pi's append-only session entries, not the temporary control file.
 * Unscoped legacy entries can be migrated from the current session only. A fork must
 * never adopt a parent's unscoped legacy Goal.
 */
export function latestGoalFromEntries(
  entries: Iterable<unknown>, sessionId: string, allowLegacy = true,
): GoalControl | undefined {
  let latest: GoalControl | undefined;
  for (const raw of entries) {
    if (!raw || typeof raw !== "object") continue;
    const entry = raw as { type?: unknown; customType?: unknown; data?: unknown };
    if (entry.type !== "custom" || entry.customType !== BRIDGE_TYPE) continue;
    if (!entry.data || typeof entry.data !== "object") continue;
    const data = entry.data as { type?: unknown; control?: unknown };
    if (data.type !== "goal_state" || !data.control || typeof data.control !== "object") continue;
    const control = data.control as Partial<GoalControl>;
    if (control.sessionId !== sessionId && !(allowLegacy && !control.sessionId)) continue;
    if (!["active", "user_paused", "blocked", "complete", "cleared"].includes(String(control.status))) continue;
    if (control.status !== "cleared" &&
      (typeof control.goalId !== "string" || !control.goalId ||
       typeof control.objective !== "string" || !control.objective)) continue;
    latest = { ...noGoal(sessionId), ...control, sessionId } as GoalControl;
  }
  return latest;
}

const BUDGET_UNSUPPORTED = "Token budgets are not enforced in this goal mode. Cannot start or resume a goal with a budget. To replace a legacy budgeted goal, clear it and create a new goal without a budget.";

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
 * (`/goal` runs outside a turn; `goal create` runs inside one).
 */
function activateGoal(
  pi: ExtensionAPI,
  sessionId: string,
  objective: string,
): { control: GoalControl; replacing: boolean } {
  const prev = readControl(sessionId);
  const replacing = !!(prev && prev.status !== "cleared");
  const control: GoalControl = {
    sessionId,
    goalId: randomUUID(),
    objective,
    status: "active",
    phase: "executing",
    tokenBaseline: 0,
    tokensUsed: 0,
    createdAt: new Date().toISOString(),
    lastEvent: replacing ? "goal_replaced" : "goal_created",
  };
  writeControl(control);
  emitBridge(pi, control, replacing ? "goal_replaced" : "goal_created");
  return { control, replacing };
}

function resumeGoal(pi: ExtensionAPI, sessionId: string): { control?: GoalControl; reason?: "active" | "inactive" | "budget_unsupported" } {
  const c = readControl(sessionId);
  if (!c || !c.goalId || !c.objective || !["user_paused", "blocked", "cleared"].includes(c.status)) {
    return { reason: c?.status === "active" ? "active" : "inactive" };
  }
  if (c.tokenBudget !== undefined && c.tokenBudget !== null) return { reason: "budget_unsupported" };
  // Cleared goals need a new generation to bypass the Pager tombstone.
  const fromCleared = c.status === "cleared";
  if (fromCleared) c.goalId = randomUUID();
  c.status = "active";
  c.phase = "executing";
  c.pauseMessage = undefined;
  c.lastEvent = "goal_resumed";
  c.lastEventDetail = fromCleared ? "restored cleared goal" : undefined;
  writeControl(c);
  emitBridge(pi, c, "goal_resumed");
  return { control: c };
}

function parseSetArgs(args: string): { objective: string; hasBudget: boolean } {
  const tokens = args.trim().split(/\s+/).filter(Boolean);
  return {
    objective: tokens.join(" "),
    hasBudget: tokens.some((token) => token === "--budget" || token.startsWith("--budget=")),
  };
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
          'Call goal({ action: "create", objective: "..." }) when the user explicitly asks you to set a goal.',
          'Call goal({ action: "update", completed: true, message: "..." }) ONLY when done.',
          'Call goal({ action: "update", blocked_reason: "..." }) only after repeated failures.',
          'Optional progress: goal({ action: "update", message: "..." }).',
          "Do not stop early. Do not invent success. Continue now.",
        ].join("\n")
      : control.status === "blocked" || control.status === "user_paused"
        ? 'Goal mode is paused. Do not continue autonomous goal work until the user explicitly asks to resume it; then call goal({ action: "resume" }).'
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

type GoalToolAction = "create" | "get" | "update" | "resume";

type GoalToolParams = {
  action?: GoalToolAction;
  objective?: string;
  token_budget?: number;
  completed?: boolean;
  message?: string;
  blocked_reason?: string;
};

type GoalToolResult = {
  content: Array<{ type: "text"; text: string }>;
  details: Record<string, unknown>;
};

function goalGet(ctx: ExtensionContext): GoalToolResult {
  const control = readControl(ctx.sessionManager.getSessionId());
  if (!control?.goalId || !control.objective) {
    return { content: [{ type: "text", text: "No goal is currently set." }], details: { ok: true, goal: null } };
  }
  const lines = [
    `Objective: ${control.objective}`,
    `Status: ${control.status}`,
    `Phase: ${control.phase}`,
    control.pauseMessage ? `Pause message: ${control.pauseMessage}` : undefined,
    control.lastEvent ? `Last event: ${control.lastEvent}${control.lastEventDetail ? ` (${control.lastEventDetail})` : ""}` : undefined,
  ].filter((line): line is string => Boolean(line));
  return { content: [{ type: "text", text: lines.join("\n") }], details: { ok: true, goal: control } };
}

function goalCreate(pi: ExtensionAPI, ctx: ExtensionContext, params: GoalToolParams): GoalToolResult {
  const sessionId = ctx.sessionManager.getSessionId();
  const objective = params.objective?.trim() ?? "";
  const invalid = validateObjective(objective);
  if (invalid) {
    return {
      content: [{ type: "text", text: `Goal not created: ${invalid}.` }],
      details: { ok: false, reason: "invalid_objective" },
    };
  }

  if (params.token_budget !== undefined) {
    return {
      content: [{ type: "text", text: BUDGET_UNSUPPORTED }],
      details: { ok: false, reason: "budget_unsupported" },
    };
  }

  const prev = readControl(sessionId);
  if (isUnfinished(prev)) {
    return {
      content: [
        {
          type: "text",
          text: `Cannot replace unfinished (${prev.status}) goal: ${prev.objective}
Use goal({ action: "resume" }) only if the user explicitly asks to resume this goal, or ask them to use /goal clear first.`,
        },
      ],
      details: { ok: false, reason: "goal_in_progress" },
    };
  }

  const { control, replacing } = activateGoal(pi, sessionId, objective);
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
          'Continue working toward it; call goal({ action: "update" }) only when done or blocked.',
        ]
          .filter((line): line is string => Boolean(line))
          .join("\n"),
      },
    ],
    details: { ok: true, status: "active", goalId: control.goalId },
  };
}

function goalResume(pi: ExtensionAPI, ctx: ExtensionContext): GoalToolResult {
  const { control, reason } = resumeGoal(pi, ctx.sessionManager.getSessionId());
  if (!control) {
    return {
      content: [{ type: "text", text: reason === "budget_unsupported" ? BUDGET_UNSUPPORTED : reason === "active" ? "Goal is already active." : "No recoverable goal to resume." }],
      details: { ok: false, reason },
    };
  }
  // The model is already in a turn; do not enqueue a second turn.
  sendGoalMessage(pi, control, "goal_resumed", undefined, { triggerTurn: false });
  return {
    content: [{ type: "text", text: `Goal resumed: ${control.objective}\nContinue working toward it.` }],
    details: { ok: true, status: "active", goalId: control.goalId },
  };
}

function goalUpdate(pi: ExtensionAPI, ctx: ExtensionContext, params: GoalToolParams): GoalToolResult {
  const c = readControl(ctx.sessionManager.getSessionId());
  if (!c || c.status === "cleared" || c.status === "complete") {
    return {
      content: [
        {
          type: "text",
          text: "Goal mode is not active (no /goal run). goal update has no effect.",
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
          text: `Goal is ${c.status}. If the user explicitly asks to resume it, call goal({ action: "resume" }) before updating it.`,
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
}

export default function piGrokGoal(pi: ExtensionAPI) {
  if (process.env.PI_GROK !== "1") return;
  if (!controlPath()) return;

  // The adapter cannot append Pi entries itself: it only owns the IPC control
  // file. It invokes PERSIST_COMMAND after an adapter-originated transition
  // (continuation loop guard); every other lifecycle point below is a fallback
  // for a missed command, a reload, or a session switch.
  const persistGuard = (ctx: ExtensionContext | ExtensionCommandContext, expectedGoalId?: string) => {
    const sessionId = ctx.sessionManager.getSessionId();
    const control = readControl(sessionId);
    if (control?.status !== "blocked" || control.lastEventDetail !== "loop_guard") return false;
    if (expectedGoalId && control.goalId !== expectedGoalId) return false;
    const durable = latestGoalFromEntries(ctx.sessionManager.getBranch(), sessionId, false);
    if (durable?.goalId !== control.goalId || durable.status !== "active") return false;
    emitBridge(pi, control, "goal_paused", "loop_guard");
    return true;
  };

  pi.registerCommand(PERSIST_COMMAND, {
    description: "Internal: journal the adapter-written goal control into the Pi session log.",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      persistGuard(ctx, args.trim() || undefined);
    },
  });

  pi.on("session_shutdown", (_event, ctx) => { persistGuard(ctx); });

  pi.on("session_start", (event, ctx) => {
    // A reload keeps this process's control file; journal an adapter block
    // before branch restoration can overwrite it.
    persistGuard(ctx);
    const sessionId = ctx.sessionManager.getSessionId();
    const restored = latestGoalFromEntries(
      ctx.sessionManager.getBranch(), sessionId, event.reason !== "fork",
    );
    const control = restored ?? noGoal(sessionId);
    if (control.status === "active") {
      // Resuming a session is not permission to restart background execution.
      control.status = "user_paused";
      control.phase = "idle";
      control.pauseMessage = "Recovered from session. Use /goal resume to continue.";
      control.lastEvent = "goal_paused";
      control.lastEventDetail = "recovered";
    }
    writeControl(control);
    // A brand-new session with no Goal remains empty. A fork is different:
    // write a scoped cleared tombstone to override any copied legacy parent
    // Goal records on a later restart of the forked child.
    if (restored || event.reason === "fork") {
      emitBridge(pi, control, control.lastEvent ?? "goal_restored");
    }
  });

  pi.on("session_before_tree", (_event, ctx) => { persistGuard(ctx); });
  pi.on("session_before_switch", (_event, ctx) => { persistGuard(ctx); });
  pi.on("session_tree", (_event, ctx) => {
    // Journal an adapter block onto the branch that is becoming current before
    // deriving from it; otherwise an active snapshot would mask the guard.
    persistGuard(ctx);
    const sessionId = ctx.sessionManager.getSessionId();
    const control = latestGoalFromEntries(ctx.sessionManager.getBranch(), sessionId, false) ?? noGoal(sessionId);
    if (control.status === "active") {
      control.status = "user_paused";
      control.phase = "idle";
      control.pauseMessage = "Recovered from session. Use /goal resume to continue.";
      control.lastEvent = "goal_paused";
      control.lastEventDetail = "recovered";
    }
    writeControl(control);
    // A tree switch is an authoritative snapshot, even for an empty branch.
    emitBridge(pi, control, "goal_tree_restored");
  });

  pi.on("context", (event, ctx) => {
    persistGuard(ctx);
    const messages = event.messages.filter((message) =>
      message.role !== "custom" || message.customType !== GOAL_MESSAGE_TYPE);
    const control = readControl(ctx.sessionManager.getSessionId());
    if (control?.goalId && control.objective) {
      messages.push({ role: "custom", customType: GOAL_MESSAGE_TYPE,
        content: goalStateReminder(control, (control.lastEvent as GoalEvent) || "progress", control.lastEventDetail),
        display: false, timestamp: Date.now() });
    }
    return { messages };
  });

  pi.on("agent_settled", (event, ctx) => {
    persistGuard(ctx);
    if (!event.aborted) return;
    const c = readControl(ctx.sessionManager.getSessionId());
    // The adapter can already have set a provisional cancelled pause before
    // Pi finishes aborting. Still append it into Pi's durable JSONL once.
    if (!c || (c.status !== "active" &&
        !(c.status === "user_paused" && c.lastEventDetail === "cancelled"))) return;
    c.status = "user_paused";
    c.phase = "idle";
    c.pauseMessage = "Execution cancelled. Use /goal resume to continue.";
    c.lastEvent = "goal_paused";
    c.lastEventDetail = "cancelled";
    writeControl(c);
    emitBridge(pi, c, "goal_paused", "cancelled");
  });

  pi.registerCommand("goal", {
    description: "Manage a session-scoped autonomous goal (status|pause|cancel|resume|clear)",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const sessionId = ctx.sessionManager.getSessionId();
      const trimmed = args.trim();
      const sub = trimmed.split(/\s+/)[0]?.toLowerCase() ?? "";

      if (!trimmed || sub === "help") {
        ctx.ui.notify(
          "Usage: /goal <objective> [--budget N] | status | pause | cancel | resume | clear",
          "info",
        );
        return;
      }

      if (sub === "status") {
        const c = readControl(sessionId);
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

      if (sub === "pause" || sub === "cancel") {
        const c = readControl(sessionId);
        if (!c || c.status !== "active") {
          ctx.ui.notify("No active goal to pause.", "warning");
          return;
        }
        c.status = "user_paused";
        c.phase = "idle";
        c.lastEvent = "goal_paused";
        c.lastEventDetail = sub === "cancel" ? "cancelled" : "user";
        writeControl(c);
        emitBridge(pi, c, "goal_paused", c.lastEventDetail);
        sendGoalMessage(pi, c, "goal_paused", c.lastEventDetail, { triggerTurn: false });
        ctx.ui.notify("Goal paused (recoverable). Use /goal resume to continue.", "info");
        return;
      }

      if (sub === "resume") {
        const { control: c, reason } = resumeGoal(pi, sessionId);
        if (!c) {
          ctx.ui.notify(
            reason === "budget_unsupported" ? BUDGET_UNSUPPORTED : reason === "active" ? "Goal is already active." : "No recoverable goal to resume.",
            reason === "active" ? "info" : "warning",
          );
          return;
        }
        sendGoalMessage(pi, c, "goal_resumed", undefined, {
          triggerTurn: true,
          deliverAs: "followUp",
        });
        return;
      }

      if (sub === "clear") {
        const c = readControl(sessionId);
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
        ctx.ui.notify("Goal cleared. /goal resume restores it until another goal replaces it.", "info");
        return;
      }

      const { objective, hasBudget } = parseSetArgs(trimmed);
      if (hasBudget) {
        ctx.ui.notify(BUDGET_UNSUPPORTED, "warning");
        return;
      }
      if (!objective) {
        ctx.ui.notify("Usage: /goal <objective> [--budget N]", "warning");
        return;
      }

      const invalid = validateObjective(objective);
      if (invalid) {
        ctx.ui.notify(invalid, "warning");
        return;
      }

      const existing = readControl(sessionId);
      if (isUnfinished(existing)) {
        ctx.ui.notify("Unfinished goal exists. Use /goal resume or /goal clear before setting a new goal.", "warning");
        return;
      }
      const { control, replacing } = activateGoal(pi, sessionId, objective);
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
    name: "goal",
    label: "Goal",
    description:
      "Manage this session's autonomous goal. action selects one operation and only that operation's parameters are used: " +
      "create { objective } starts an active goal when none is unfinished; " +
      "get returns the current goal state; " +
      "update { message?, completed?, blocked_reason? } reports progress, completes, or blocks; " +
      "resume continues a paused, blocked, or cleared goal when the user explicitly asks. " +
      "Token budgets are unsupported and rejected.",
    promptSnippet: "Manage the session goal with goal({ action: create|get|update|resume }).",
    promptGuidelines: [
      'Call goal({ action: "create" }) only when the user explicitly asks to set a goal.',
      'Call goal({ action: "update", completed: true }) only when the goal is fully achieved with evidence.',
      'Call goal({ action: "resume" }) only when the user explicitly asks to continue a paused goal.',
    ],
    parameters: Type.Object({
      action: Type.Union(
        [
          Type.Literal("create"),
          Type.Literal("get"),
          Type.Literal("update"),
          Type.Literal("resume"),
        ],
        { description: "Operation to run; only its parameters are used." },
      ),
      objective: Type.Optional(
        Type.String({
          description: "Goal objective (create only; required for create).",
        }),
      ),
      token_budget: Type.Optional(
        Type.Integer({
          description: "Unsupported: any supplied token budget is rejected (create only).",
        }),
      ),
      completed: Type.Optional(
        Type.Boolean({
          description: "Set true ONLY when the goal is fully achieved (update only). Ends goal mode.",
        }),
      ),
      message: Type.Optional(
        Type.String({
          description: "Short progress or completion summary (update only).",
        }),
      ),
      blocked_reason: Type.Optional(
        Type.String({
          description: "Set only when truly stuck after repeated failures; pauses the goal (update only).",
        }),
      ),
    }),
    async execute(
      _toolCallId: string,
      params: GoalToolParams,
      _signal: AbortSignal | undefined,
      _onUpdate: unknown,
      ctx: ExtensionContext,
    ): Promise<GoalToolResult> {
      switch (params.action) {
        case "create":
          return goalCreate(pi, ctx, params);
        case "get":
          return goalGet(ctx);
        case "resume":
          return goalResume(pi, ctx);
        case "update":
          return goalUpdate(pi, ctx, params);
        default:
          return {
            content: [
              {
                type: "text",
                text: `Goal unchanged: unknown action "${String(params.action)}". Use create, get, update, or resume.`,
              },
            ],
            details: { ok: false, reason: "invalid_action" },
          };
      }
    },
  });
}
