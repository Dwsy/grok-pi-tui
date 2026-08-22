import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

// v1: Grok-native todo_write semantics — merge/replace by string id, no steering.

type V1Status = "pending" | "in_progress" | "completed" | "cancelled";
type V1Update = { id: string; content?: string; status?: V1Status };
type V1Item = { id: string; content: string; status: V1Status };

const V1_STATUSES: readonly V1Status[] = ["pending", "in_progress", "completed", "cancelled"];

const V1_STATUS = Type.Union([
  Type.Literal("pending"),
  Type.Literal("in_progress"),
  Type.Literal("completed"),
  Type.Literal("cancelled"),
]);

const V1Parameters = Type.Object({
  merge: Type.Optional(
    Type.Boolean({
      description:
        "Optional. When true (default), merges the provided todos into the existing list by id — send only the items you are changing, and to flip status without changing content send just id + status. When false, the provided todos replace the existing list.",
    }),
  ),
  todos: Type.Array(
    Type.Object({
      id: Type.String({ description: "Unique identifier for the todo item" }),
      content: Type.Optional(Type.String({ description: "The description/content of the todo item" })),
      status: Type.Optional(V1_STATUS),
    }),
    { description: "Array of todo items to write to the workspace" },
  ),
});

function isV1Snapshot(value: unknown): value is { todos: unknown[] } {
  return !!value && typeof value === "object" && Array.isArray((value as { todos?: unknown }).todos);
}

// Foreign (v2/legacy) snapshots carry `tasks` + numeric `nextId` without the
// v1 `todos` marker. Cross-version switching migrates the latest such
// snapshot instead of starting empty.
function isForeignTodoSnapshot(value: unknown): value is { tasks: unknown[] } {
  if (!value || typeof value !== "object" || isV1Snapshot(value)) return false;
  const snapshot = value as { tasks?: unknown; nextId?: unknown; version?: unknown };
  if (snapshot.version === 1) return false;
  return Array.isArray(snapshot.tasks) && typeof snapshot.nextId === "number";
}

// v2 tasks → v1 items: subject becomes content, tombstoned tasks are dropped.
function migrateV2TasksToV1(tasks: unknown[]): V1Item[] {
  const items: V1Item[] = [];
  for (const raw of tasks) {
    if (!raw || typeof raw !== "object") continue;
    const task = raw as { id?: unknown; subject?: unknown; status?: unknown };
    const id = task.id !== undefined && task.id !== null && String(task.id) !== "" ? String(task.id) : "";
    if (!id) continue;
    const subject = typeof task.subject === "string" ? task.subject.trim() : "";
    if (!subject) continue;
    if (task.status === "deleted") continue;
    const status =
      task.status === "in_progress" || task.status === "completed" ? task.status : "pending";
    items.push({ id, content: subject, status });
  }
  return items;
}

function sanitizeV1Items(entries: unknown[]): V1Item[] {
  const items: V1Item[] = [];
  for (const entry of entries) {
    if (!entry || typeof entry !== "object") continue;
    const raw = entry as { id?: unknown; content?: unknown; status?: unknown };
    if (typeof raw.id !== "string" || raw.id.length === 0) continue;
    const content = typeof raw.content === "string" ? raw.content : raw.id;
    const status = V1_STATUSES.find((candidate) => candidate === raw.status) ?? "pending";
    items.push({ id: raw.id, content, status });
  }
  return items;
}

function v1StateFromBranch(ctx: { sessionManager: { getBranch(): Iterable<unknown> } }): V1Item[] {
  let items: V1Item[] = [];
  for (const entry of ctx.sessionManager.getBranch()) {
    const item = entry as { type?: string; message?: { role?: string; toolName?: string; details?: unknown } };
    if (item.type !== "message") continue;
    const message = item.message;
    if (message?.role !== "toolResult" || message.toolName !== "todo") continue;
    // The latest snapshot in the branch wins, whichever version wrote it.
    if (isV1Snapshot(message.details)) items = sanitizeV1Items(message.details.todos);
    else if (isForeignTodoSnapshot(message.details)) items = migrateV2TasksToV1(message.details.tasks);
  }
  return items;
}

function v1HasContent(update: V1Update): boolean {
  return typeof update.content === "string" && update.content.length > 0;
}

function v1DuplicateId(updates: V1Update[]): string | undefined {
  const seen = new Set<string>();
  for (const update of updates) {
    if (seen.has(update.id)) return update.id;
    seen.add(update.id);
  }
  return undefined;
}

function applyV1Replace(items: V1Item[], updates: V1Update[]): V1Item[] {
  return updates.map((update) => ({
    id: update.id,
    content: v1HasContent(update) ? update.content! : update.id,
    status: update.status ?? "pending",
  }));
}

function applyV1Merge(items: V1Item[], updates: V1Update[]): V1Item[] {
  const next = items.map((item) => ({ ...item }));
  for (const update of updates) {
    const existing = next.find((item) => item.id === update.id);
    if (existing) {
      if (v1HasContent(update)) existing.content = update.content!;
      if (update.status) existing.status = update.status;
      continue;
    }
    next.push({
      id: update.id,
      content: v1HasContent(update) ? update.content! : update.id,
      status: update.status ?? "pending",
    });
  }
  return next;
}

function summarizeV1(items: V1Item[]): string {
  if (items.length === 0) return "No tasks currently tracked.";
  return items.map((item) => `- [${item.status}] ${item.id}: ${item.content}`).join("\n");
}

// `tasks` mirrors the snapshot shape projected onto the native TodoPane
// (`details.tasks` with subject/status); cancelled entries stay out of the
// pane. `todos` is the authoritative replay state.
function v1Result(items: V1Item[], text: string, error?: string) {
  return {
    content: [{ type: "text" as const, text }],
    details: {
      version: 1 as const,
      todos: items.map((item) => ({ ...item })),
      tasks: items
        .filter((item) => item.status !== "cancelled")
        .map((item) => ({ id: item.id, subject: item.content, status: item.status })),
      nextId: 0,
      ...(error ? { error } : {}),
    },
  };
}

export function registerV1(pi: ExtensionAPI) {
  pi.registerTool({
    name: "todo",
    label: "Todo",
    description:
      "Create and manage a structured task list. The user sees this list live — it is your primary way to show progress.\n\nUse for any task with 3+ steps. Skip for trivial single-step work.",
    promptSnippet: "Track multi-step work with the built-in structured todo list.",
    promptGuidelines: [
      "Use todo for multi-step work (3+ steps); skip trivial single-step requests.",
      "Keep one task in_progress while working and flip statuses with id-only merge updates.",
    ],
    parameters: V1Parameters,
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const input = params as { merge?: boolean; todos: V1Update[] };
      const state = v1StateFromBranch(ctx);

      const duplicate = v1DuplicateId(input.todos);
      if (duplicate) {
        return v1Result(
          state,
          `Error: Duplicate todo ID in request: "${duplicate}". Each todo item must have a unique ID.`,
          `DuplicateTodoID(${duplicate})`,
        );
      }

      // Auto-upgrade to merge when the model forgot merge:true but clearly
      // intended a partial update: state already has items and every update
      // targets an existing id without providing content.
      const effectiveMerge =
        input.merge !== false ||
        (state.length > 0 &&
          input.todos.length > 0 &&
          input.todos.every((u) => !v1HasContent(u) && state.some((item) => item.id === u.id)));
      const next = effectiveMerge ? applyV1Merge(state, input.todos) : applyV1Replace(state, input.todos);
      return v1Result(next, summarizeV1(next));
    },
  });

  pi.registerCommand("todos", {
    description: "Show the current branch's built-in todo list",
    handler: async (_args, ctx) => {
      ctx.ui.notify(summarizeV1(v1StateFromBranch(ctx)), "info");
    },
  });
}
