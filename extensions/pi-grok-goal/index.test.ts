import { afterEach, expect, mock, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Schema shape is irrelevant to lifecycle tests; the actual extension source
// is loaded unchanged and Pi's official runtime supplies these definitions.
mock.module("@sinclair/typebox", () => ({
  Type: Object.fromEntries(
    ["Object", "String", "Integer", "Boolean", "Optional"].map((key) => [
      key,
      () => ({}),
    ]),
  ),
}));

const { default: registerGoal, latestGoalFromEntries } = await import("./index.ts");

type Handler = (event: unknown, ctx: unknown) => Promise<void> | void;
const cleanup: Array<() => void> = [];
afterEach(() => {
  for (const fn of cleanup.splice(0).reverse()) fn();
});

function harness(existing?: Map<string, Array<unknown>>) {
  const directory = mkdtempSync(join(tmpdir(), "pi-goal-recovery-test-"));
  const originalGoal = process.env.PI_GROK_GOAL_CONTROL;
  const originalFlag = process.env.PI_GROK;
  process.env.PI_GROK_GOAL_CONTROL = join(directory, "control.json");
  process.env.PI_GROK = "1";
  cleanup.push(() => {
    rmSync(directory, { recursive: true, force: true });
    if (originalGoal === undefined) delete process.env.PI_GROK_GOAL_CONTROL;
    else process.env.PI_GROK_GOAL_CONTROL = originalGoal;
    if (originalFlag === undefined) delete process.env.PI_GROK;
    else process.env.PI_GROK = originalFlag;
  });

  const entries = existing ?? new Map<string, Array<unknown>>();
  let current = "session-A";
  const hooks = new Map<string, Handler>();
  const tools = new Map<string, any>();
  const commands = new Map<string, any>();
  const notices: string[] = [];
  const messages: unknown[] = [];
  const pi = {
    on: (event: string, handler: Handler) => {
      hooks.set(event, handler);
      return () => {};
    },
    registerCommand: (name: string, def: unknown) => commands.set(name, def),
    registerTool: (def: { name: string }) => tools.set(def.name, def),
    appendEntry: (customType: string, data: unknown) => {
      const values = entries.get(current) ?? [];
      values.push({ type: "custom", customType, data });
      entries.set(current, values);
    },
    sendMessage: (msg: unknown) => {
      messages.push(msg);
    },
  };

  registerGoal(pi as never);
  const context = () => ({
    sessionManager: {
      getSessionId: () => current,
      getEntries: () => entries.get(current) ?? [],
    },
    ui: {
      notify: (message: string) => notices.push(message),
    },
  });
  return {
    entries,
    messages,
    notices,
    current: () => current,
    switchTo: (id: string) => {
      current = id;
    },
    control: () => JSON.parse(readFileSync(process.env.PI_GROK_GOAL_CONTROL!, "utf8")),
    start: (reason = "startup") => hooks.get("session_start")!({ type: "session_start", reason }, context()),
    settle: (aborted: boolean) => hooks.get("agent_settled")!({ type: "agent_settled", aborted }, context()),
    command: async (args: string) => commands.get("goal")!.handler(args, context()),
    create: async (objective: string) =>
      tools.get("create_goal")!.execute("call", { objective }, undefined, undefined, context()),
    update: async (params: unknown) =>
      tools.get("update_goal")!.execute("call", params, undefined, undefined, context()),
  };
}

test("Goal is scoped to the active Pi session; A never appears in B", async () => {
  const h = harness();
  await h.start();
  await h.command("ship A");
  const idA = h.control().goalId;
  expect(h.control().sessionId).toBe("session-A");
  h.switchTo("session-B");
  await h.start("new");
  expect(h.control().status).toBe("cleared");
  expect(h.control().sessionId).toBe("session-B");
  await h.command("ship B");
  expect(h.control().objective).toBe("ship B");
  expect(h.control().goalId).not.toBe(idA);
  h.switchTo("session-A");
  await h.start("resume");
  expect(h.control().goalId).toBe(idA);
  expect(h.control().status).toBe("user_paused");
  await h.command("resume");
  expect(h.control().status).toBe("active");
});

test("Process restart rehydrates durable entries but requires explicit resume", async () => {
  const sessionEntries = new Map<string, Array<unknown>>();
  const a = harness(sessionEntries);
  await a.start();
  await a.command("repair-after-restart");
  const goalId = a.control().goalId;
  // New process: the new Extension instance has a new, empty control file.
  const b = harness(sessionEntries);
  await b.start("startup");
  expect(b.control().goalId).toBe(goalId);
  expect(b.control().status).toBe("user_paused");
  expect(b.control().lastEventDetail).toBe("recovered");
  await b.command("resume");
  expect(b.control().status).toBe("active");
});

test("Both slash and create_goal reject overwriting paused and blocked goals", async () => {
  const h = harness();
  await h.start();
  await h.command("first");
  await h.command("pause");
  const firstId = h.control().goalId;
  await h.command("second");
  expect(h.control().goalId).toBe(firstId);
  const rejected = await h.create("second");
  expect(rejected.details.reason).toBe("goal_in_progress");
  await h.command("resume");
  await h.update({ blocked_reason: "tests not ready" });
  expect(h.control().status).toBe("blocked");
  expect((await h.create("second")).details.reason).toBe("goal_in_progress");
  await h.command("second");
  expect(h.control().goalId).toBe(firstId);
  await h.command("resume");
  await h.command("cancel");
  expect(h.control().status).toBe("user_paused");
  expect(h.control().goalId).toBe(firstId);
});

test("Ctrl+C soft cancellation and clear remain resumable", async () => {
  const h = harness();
  await h.start();
  await h.command("repair cabinet");
  const goalId = h.control().goalId;
  await h.settle(true);
  expect(h.control().status).toBe("user_paused");
  expect(h.control().lastEventDetail).toBe("cancelled");
  await h.command("resume");
  expect(h.control().status).toBe("active");
  await h.command("clear");
  expect(h.control().status).toBe("cleared");
  await h.command("resume");
  expect(h.control().goalId).not.toBe(goalId);
  expect(h.control().objective).toBe("repair cabinet");
  expect(h.control().status).toBe("active");
});

test("Restoration rejects parent session id and legacy parent entries on fork", () => {
  const old = {
    type: "custom",
    customType: "pi-grok-goal/v1",
    data: {
      type: "goal_state",
      control: { goalId: "parent", objective: "parent goal", status: "active" },
    },
  };
  expect(latestGoalFromEntries([old], "child", false)).toBeUndefined();
  expect(latestGoalFromEntries([old], "child", true)?.sessionId).toBe("child");
  const tagged = {
    ...old,
    data: {
      type: "goal_state",
      control: { sessionId: "parent", goalId: "parent", objective: "parent goal", status: "active" },
    },
  };
  expect(latestGoalFromEntries([tagged], "child", true)).toBeUndefined();
});


test("Fork tombstone blocks legacy parent resurrection on a later restart", async () => {
  const legacy = {
    type: "custom", customType: "pi-grok-goal/v1",
    data: { type: "goal_state", control: {
      goalId: "legacy-parent", objective: "parent work", status: "active",
    } },
  };
  const sessions = new Map<string, Array<unknown>>([
    ["child", [legacy]],
  ]);
  const first = harness(sessions);
  first.switchTo("child");
  await first.start("fork");
  expect(first.control().status).toBe("cleared");
  const next = harness(sessions);
  next.switchTo("child");
  await next.start("startup");
  expect(next.control().status).toBe("cleared");
  expect(next.control().goalId).toBe("");
});
