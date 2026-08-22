import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

// v2: rich action-based todo — create/update/list/get/delete/clear, blockedBy
// dependencies, backing events, mid-run nudges and completion reminders.

type TaskStatus = "pending" | "in_progress" | "completed" | "deleted";
type TaskAction = "create" | "update" | "list" | "get" | "delete" | "clear";

type Task = {
  id: number;
  subject: string;
  description?: string;
  activeForm?: string;
  status: TaskStatus;
  blockedBy?: number[];
  owner?: string;
  metadata?: Record<string, unknown>;
};

type TaskState = { tasks: Task[]; nextId: number };
type Params = {
  action: TaskAction;
  subject?: string;
  description?: string;
  activeForm?: string;
  status?: TaskStatus;
  blockedBy?: number[];
  addBlockedBy?: number[];
  removeBlockedBy?: number[];
  owner?: string;
  metadata?: Record<string, unknown>;
  id?: number;
  includeDeleted?: boolean;
};

const EMPTY_STATE: TaskState = { tasks: [], nextId: 1 };
const TODO_BACKING_EVENT = "pi-grok:todo-backing";
const MID_RUN_NUDGE_MUTATION_THRESHOLD = 12;
const MID_RUN_NUDGE_MAX_PER_CYCLE = 2;
const COMPLETION_REMINDER_MAX_PER_CYCLE = 2;
const MUTATING_TOOLS = new Set(["bash", "eval", "edit", "write", "ast_edit"]);
const STATUS = Type.Union([
  Type.Literal("pending"),
  Type.Literal("in_progress"),
  Type.Literal("completed"),
  Type.Literal("deleted"),
]);
const Parameters = Type.Object({
  action: Type.Union([
    Type.Literal("create"),
    Type.Literal("update"),
    Type.Literal("list"),
    Type.Literal("get"),
    Type.Literal("delete"),
    Type.Literal("clear"),
  ]),
  subject: Type.Optional(Type.String({ description: "Task subject line (required for create)." })),
  description: Type.Optional(Type.String({ description: "Long-form task description." })),
  activeForm: Type.Optional(Type.String({ description: "Present-continuous label while in_progress, e.g. 'writing tests'." })),
  status: Type.Optional(STATUS),
  blockedBy: Type.Optional(Type.Array(Type.Number(), { description: "Initial dependency ids (create only)." })),
  addBlockedBy: Type.Optional(Type.Array(Type.Number(), { description: "Dependency ids to add (update only)." })),
  removeBlockedBy: Type.Optional(Type.Array(Type.Number(), { description: "Dependency ids to remove (update only)." })),
  owner: Type.Optional(Type.String({ description: "Optional task owner." })),
  metadata: Type.Optional(Type.Record(Type.String(), Type.Unknown())),
  id: Type.Optional(Type.Number({ description: "Task id (required for update/get/delete)." })),
  includeDeleted: Type.Optional(Type.Boolean({ description: "Include deleted tombstones when listing." })),
});

function isSnapshot(value: unknown): value is { tasks: Task[]; nextId: number } {
  if (!value || typeof value !== "object") return false;
  const snapshot = value as { tasks?: unknown; nextId?: unknown; version?: unknown };
  if (snapshot.version === 1) return false;
  return Array.isArray(snapshot.tasks) && typeof snapshot.nextId === "number";
}

// Foreign (v1) snapshots carry the `todos` marker. Cross-version switching
// migrates the latest such snapshot instead of starting empty.
function isForeignTodoSnapshot(value: unknown): value is { todos: unknown[] } {
  return !!value && typeof value === "object" && Array.isArray((value as { todos?: unknown }).todos);
}

// v1 items → v2 tasks: content becomes subject, string ids are renumbered,
// cancelled items become deleted tombstones so they stay out of the pane.
function migrateV1TodosToV2(todos: unknown[]): TaskState {
  const tasks: Task[] = [];
  let nextId = EMPTY_STATE.nextId;
  for (const raw of todos) {
    if (!raw || typeof raw !== "object") continue;
    const item = raw as { id?: unknown; content?: unknown; status?: unknown };
    const subject = typeof item.content === "string" ? item.content.trim() : "";
    if (!subject) continue;
    const status =
      item.status === "in_progress" || item.status === "completed"
        ? item.status
        : item.status === "cancelled"
          ? ("deleted" as const)
          : ("pending" as const);
    tasks.push({ id: nextId, subject, status });
    nextId += 1;
  }
  return { tasks, nextId };
}

function stateFromBranch(ctx: { sessionManager: { getBranch(): Iterable<unknown> } }): TaskState {
  let state: TaskState = { tasks: [], nextId: EMPTY_STATE.nextId };
  for (const entry of ctx.sessionManager.getBranch()) {
    const item = entry as { type?: string; message?: { role?: string; toolName?: string; details?: unknown } };
    if (item.type !== "message") continue;
    const message = item.message;
    if (message?.role !== "toolResult" || message.toolName !== "todo") continue;
    // The latest snapshot in the branch wins, whichever version wrote it.
    if (isSnapshot(message.details)) {
      state = {
        tasks: message.details.tasks.map((task) => ({ ...task })),
        nextId: message.details.nextId,
      };
    } else if (isForeignTodoSnapshot(message.details)) {
      state = migrateV1TodosToV2(message.details.todos);
    }
  }
  return state;
}

function result(action: TaskAction, params: Params, state: TaskState, text: string, error?: string) {
  return {
    content: [{ type: "text" as const, text }],
    details: {
      version: 2 as const,
      action,
      params,
      tasks: state.tasks,
      nextId: state.nextId,
      ...(error ? { error } : {}),
    },
  };
}

function fail(action: TaskAction, params: Params, state: TaskState, message: string) {
  return result(action, params, state, `Error: ${message}`, message);
}

function requireId(params: Params): number | undefined {
  return Number.isInteger(params.id) && (params.id ?? 0) > 0 ? params.id : undefined;
}

function formatTask(task: Task): string {
  const form = task.status === "in_progress" && task.activeForm ? ` (${task.activeForm})` : "";
  const blocked = task.blockedBy?.length ? ` blocked by ${task.blockedBy.map((id) => `#${id}`).join(", ")}` : "";
  return `[${task.status}] #${task.id} ${task.subject}${form}${blocked}`;
}

function executeMutation(params: Params, state: TaskState) {
  const action = params.action;
  if (action === "create") {
    const subject = params.subject?.trim();
    if (!subject) return fail(action, params, state, "create requires a non-empty subject");
    const task: Task = {
      id: state.nextId,
      subject,
      status: params.status ?? "pending",
      ...(params.description?.trim() ? { description: params.description.trim() } : {}),
      ...(params.activeForm?.trim() ? { activeForm: params.activeForm.trim() } : {}),
      ...(params.blockedBy?.length ? { blockedBy: [...new Set(params.blockedBy)] } : {}),
      ...(params.owner?.trim() ? { owner: params.owner.trim() } : {}),
      ...(params.metadata ? { metadata: { ...params.metadata } } : {}),
    };
    const next = { tasks: [...state.tasks, task], nextId: state.nextId + 1 };
    return result(action, params, next, `Created #${task.id}: ${task.subject} (${task.status})`);
  }

  if (action === "clear") {
    return result(action, params, { tasks: [], nextId: 1 }, `Cleared ${state.tasks.length} tasks`);
  }

  if (action === "list") {
    let tasks = params.includeDeleted ? state.tasks : state.tasks.filter((task) => task.status !== "deleted");
    if (params.status) tasks = tasks.filter((task) => task.status === params.status);
    return result(action, params, state, tasks.length ? tasks.map(formatTask).join("\n") : "No tasks");
  }

  const id = requireId(params);
  if (!id) return fail(action, params, state, `${action} requires a positive integer id`);
  const index = state.tasks.findIndex((task) => task.id === id);
  if (index < 0) return fail(action, params, state, `unknown task #${id}`);
  const current = state.tasks[index];

  if (action === "get") return result(action, params, state, formatTask(current));

  if (action === "delete") {
    const tasks = state.tasks.map((task, i) => (i === index ? { ...task, status: "deleted" as const } : task));
    return result(action, params, { ...state, tasks }, `Deleted #${id}: ${current.subject}`);
  }

  if (action === "update") {
    const blocked = new Set(current.blockedBy ?? []);
    for (const dep of params.addBlockedBy ?? []) blocked.add(dep);
    for (const dep of params.removeBlockedBy ?? []) blocked.delete(dep);
    blocked.delete(id);
    const metadata = params.metadata
      ? Object.fromEntries(
          Object.entries({ ...(current.metadata ?? {}), ...params.metadata }).filter(([, value]) => value !== null),
        )
      : current.metadata;
    const updated: Task = {
      ...current,
      ...(params.subject?.trim() ? { subject: params.subject.trim() } : {}),
      ...(params.description !== undefined ? { description: params.description.trim() || undefined } : {}),
      ...(params.activeForm !== undefined ? { activeForm: params.activeForm.trim() || undefined } : {}),
      ...(params.status ? { status: params.status } : {}),
      ...(params.owner !== undefined ? { owner: params.owner.trim() || undefined } : {}),
      ...(params.addBlockedBy || params.removeBlockedBy ? { blockedBy: [...blocked] } : {}),
      ...(params.metadata ? { metadata } : {}),
    };
    const tasks = state.tasks.map((task, i) => (i === index ? updated : task));
    return result(action, params, { ...state, tasks }, `Updated #${id} (${current.status} → ${updated.status})`);
  }

  return fail(action, params, state, `unsupported action: ${action}`);
}

async function showTodos(ctx: ExtensionCommandContext): Promise<void> {
  const visible = stateFromBranch(ctx).tasks.filter((task) => task.status !== "deleted");
  ctx.ui.notify(visible.length ? visible.map(formatTask).join("\n") : "No todos yet.", "info");
}

function textFromAssistantMessage(message: unknown): string {
  if (!message || typeof message !== "object") return "";
  const candidate = message as { role?: unknown; content?: unknown };
  if (candidate.role !== "assistant" || !Array.isArray(candidate.content)) return "";
  return candidate.content
    .filter((block): block is { type: string; text: string } => {
      if (!block || typeof block !== "object") return false;
      const value = block as { type?: unknown; text?: unknown };
      return value.type === "text" && typeof value.text === "string";
    })
    .map((block) => block.text)
    .join("")
    .trim();
}

function isAwaitingUserAnswer(text: string): boolean {
  const lastLine = text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean).at(-1);
  if (!lastLine) return false;
  if (/[?？]\s*$/.test(lastLine)) return true;
  return /^(?:please\s+)?(?:confirm|reply|choose|pick|decide|advise|answer|let\s+me\s+know|tell\s+me)\b/i.test(lastLine);
}

function backingCountFromEvent(event: unknown): { source: string; count: number } | undefined {
  if (!event || typeof event !== "object") return undefined;
  const value = event as { source?: unknown; count?: unknown };
  if (typeof value.source !== "string" || typeof value.count !== "number" || !Number.isFinite(value.count)) return undefined;
  return { source: value.source, count: Math.max(0, Math.floor(value.count)) };
}

function gateState(state: TaskState, backingTaskCount: number): {
  pending: Task[];
  inProgressBacked: Task[];
  inProgressUnbacked: Task[];
} {
  const pending = state.tasks.filter((task) => task.status === "pending");
  const inProgress = state.tasks.filter((task) => task.status === "in_progress");
  const backedCount = Math.min(inProgress.length, Math.max(0, backingTaskCount));
  return {
    pending,
    inProgressBacked: inProgress.slice(0, backedCount),
    inProgressUnbacked: inProgress.slice(backedCount),
  };
}

function formatCompletionReminder(pending: Task[], inProgressUnbacked: Task[], attempt: number): string {
  const lines = ["<system-reminder>", "You stopped with actionable todos still open."];
  if (inProgressUnbacked.length > 0) {
    lines.push("", "In progress without a backing background task:");
    lines.push(...inProgressUnbacked.map((task) => `- #${task.id} ${task.subject}`));
  }
  if (pending.length > 0) {
    lines.push("", "Pending:");
    lines.push(...pending.map((task) => `- #${task.id} ${task.subject}`));
  }
  lines.push(
    "",
    "Continue working on these tasks, or update the todo list if work is finished or no longer actionable.",
    `(Reminder ${attempt}/${COMPLETION_REMINDER_MAX_PER_CYCLE})`,
    "</system-reminder>",
  );
  return lines.join("\n");
}

export function registerV2(pi: ExtensionAPI) {
  let mutationsSinceLastTodo = 0;
  let midRunNudgeCount = 0;
  let completionReminderCount = 0;
  let completionReminderAwaitingProgress = false;
  let lastAssistantText = "";
  const backingCounts = new Map<string, number>();

  const resetCycle = () => {
    mutationsSinceLastTodo = 0;
    midRunNudgeCount = 0;
    completionReminderCount = 0;
    completionReminderAwaitingProgress = false;
  };

  const totalBackingCount = () => [...backingCounts.values()].reduce((sum, count) => sum + count, 0);

  pi.events.on(TODO_BACKING_EVENT, (event) => {
    const update = backingCountFromEvent(event);
    if (!update) return;
    backingCounts.set(update.source, update.count);
  });

  pi.on("session_start", () => {
    backingCounts.clear();
    lastAssistantText = "";
    resetCycle();
  });

  pi.on("input", (event) => {
    if (event.source !== "extension") resetCycle();
  });

  pi.on("message_end", (event) => {
    const message = event.message as { role?: unknown };
    if (message.role === "assistant") lastAssistantText = textFromAssistantMessage(event.message);
  });

  pi.on("turn_end", (event, ctx) => {
    if (event.toolResults.length === 0) return;
    completionReminderAwaitingProgress = false;

    let nudgeDue = false;
    for (const toolResult of event.toolResults) {
      if (toolResult.toolName === "todo") {
        mutationsSinceLastTodo = 0;
        nudgeDue = false;
        continue;
      }
      if (toolResult.isError || !MUTATING_TOOLS.has(toolResult.toolName)) continue;
      mutationsSinceLastTodo += 1;
      if (mutationsSinceLastTodo >= MID_RUN_NUDGE_MUTATION_THRESHOLD) nudgeDue = true;
    }

    if (!nudgeDue) return;
    if (midRunNudgeCount >= MID_RUN_NUDGE_MAX_PER_CYCLE) return;

    const state = stateFromBranch(ctx);
    const incomplete = state.tasks.filter((task) => task.status === "pending" || task.status === "in_progress");
    if (incomplete.length === 0) return;

    mutationsSinceLastTodo -= MID_RUN_NUDGE_MUTATION_THRESHOLD;
    midRunNudgeCount += 1;
    // Queue only after the entire tool batch is finalized. Steering from tool_execution_end can split
    // assistant tool calls from their real tool results, causing Pi to synthesize duplicate outputs.
    pi.sendMessage(
      {
        customType: "pi-grok-todo-mid-run-nudge/v1",
        content:
          `<system-reminder>\n${incomplete.length} todo item${incomplete.length === 1 ? "" : "s"} still open. ` +
          "If you finished a task since the last todo update, mark it completed now so progress stays visible; otherwise keep working.\n</system-reminder>",
        display: false,
        details: { nudge: midRunNudgeCount, incomplete: incomplete.length },
      },
      { triggerTurn: false, deliverAs: "steer" },
    );
  });

  pi.on("agent_settled", (_event, ctx) => {
    if (!ctx.isIdle() || ctx.hasPendingMessages()) return;
    if (completionReminderAwaitingProgress) return;
    if (completionReminderCount >= COMPLETION_REMINDER_MAX_PER_CYCLE) return;

    const state = stateFromBranch(ctx);
    const gate = gateState(state, totalBackingCount());
    if (gate.pending.length === 0 && gate.inProgressUnbacked.length === 0) {
      completionReminderCount = 0;
      completionReminderAwaitingProgress = false;
      return;
    }
    if (isAwaitingUserAnswer(lastAssistantText)) return;

    completionReminderCount += 1;
    completionReminderAwaitingProgress = true;
    mutationsSinceLastTodo = 0;
    pi.sendMessage(
      {
        customType: "pi-grok-todo-completion-reminder/v1",
        content: formatCompletionReminder(gate.pending, gate.inProgressUnbacked, completionReminderCount),
        display: false,
        details: {
          attempt: completionReminderCount,
          pending: gate.pending.length,
          inProgressBacked: gate.inProgressBacked.length,
          inProgressUnbacked: gate.inProgressUnbacked.length,
          backingTaskCount: totalBackingCount(),
        },
      },
      { triggerTurn: true, deliverAs: "followUp" },
    );
  });

  pi.registerTool({
    name: "todo",
    label: "Todo",
    description:
      "Manage the current session's structured task list. Use create/update/list/get/delete/clear to track multi-step work; keep one task in_progress while working and mark it completed immediately when done.",
    promptSnippet: "Track multi-step work with the built-in structured todo list.",
    promptGuidelines: [
      "Use todo for multi-step work rather than for trivial one-step requests.",
      "Keep roughly one task in_progress; mark completed tasks immediately.",
      "Do not mark work completed while tests fail or a blocker remains unresolved.",
    ],
    parameters: Parameters,
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      return executeMutation(params as Params, stateFromBranch(ctx));
    },
  });

  pi.registerCommand("todos", {
    description: "Show the current branch's built-in todo list",
    handler: async (_args, ctx) => showTodos(ctx),
  });
}
