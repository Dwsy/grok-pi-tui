import { afterEach, expect, mock, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Schema shape is irrelevant to lifecycle tests; the actual extension source
// is loaded unchanged and Pi's official runtime supplies these definitions.
mock.module("@sinclair/typebox", () => ({
  Type: Object.fromEntries(
    ["Object", "String", "Integer", "Boolean", "Optional", "Union", "Literal"].map((key) => [
      key,
      () => ({}),
    ]),
  ),
}));

const { default: registerGoal, latestGoalFromEntries } = await import("./index.ts");

type Handler = (event: any, ctx: any) => any;
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
  let branch: Array<unknown> | undefined;
  const hooks = new Map<string, Handler>();
  const tools = new Map<string, any>();
  const commands = new Map<string, any>();
  const notices: string[] = [];
  const messages: Array<{ msg: unknown; options: any }> = [];
  const bridgeEvents: Array<{ event: any; disk: any }> = [];
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
      bridgeEvents.push({
        event: { type: "entry_appended", entry: { type: "custom", customType, data } },
        disk: JSON.parse(readFileSync(process.env.PI_GROK_GOAL_CONTROL!, "utf8")),
      });
    },
    sendMessage: (msg: unknown, options: unknown) => {
      messages.push({ msg, options });
    },
  };

  registerGoal(pi as never);
  cleanup.push(() => hooks.get("session_shutdown")?.({ type: "session_shutdown" }, context()));
  const context = () => ({
    sessionManager: {
      getSessionId: () => current,
      getEntries: () => entries.get(current) ?? [],
      getBranch: () => branch ?? entries.get(current) ?? [],
    },
    ui: {
      notify: (message: string) => notices.push(message),
    },
  });
  return {
    entries,
    bridgeEvents,
    setBranch: (value: Array<unknown>) => { branch = value; },
    tree: () => hooks.get("session_tree")!({ type: "session_tree" }, context()),
    modelContext: (messages: unknown[]) => hooks.get("context")!({ type: "context", messages }, context()) as any,
    get: async () => tools.get("goal")!.execute("call", { action: "get" }, undefined, undefined, context()),
    persist: async (goalId = "") =>
      commands.get("__pi_grok_goal_persist")!.handler(goalId, context()),
    goal: async (params: Record<string, unknown>) =>
      tools.get("goal")!.execute("call", params, undefined, undefined, context()),
    toolNames: () => [...tools.keys()],
    toolDefinition: () => tools.get("goal"),
    writeControl: (control: unknown) => writeFileSync(process.env.PI_GROK_GOAL_CONTROL!, JSON.stringify(control)),
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
    create: async (objective: string, token_budget?: number) =>
      tools.get("goal")!.execute("call", { action: "create", objective, token_budget }, undefined, undefined, context()),
    resume: async () =>
      tools.get("goal")!.execute("call", { action: "resume" }, undefined, undefined, context()),
    update: async (params: Record<string, unknown>) =>
      tools.get("goal")!.execute("call", { action: "update", ...params }, undefined, undefined, context()),
  };
}

test("budgets are rejected without creating a goal", async () => {
  const h = harness();
  await h.start();
  for (const args of ["repair --budget 100", "repair --budget", "repair --budget=100", "repair --budget nope"]) {
    await h.command(args);
    expect(h.notices.at(-1)).toContain("not enforced");
    expect(h.control().status).toBe("cleared");
  }
  for (const budget of [100, 0, -1]) {
    const result = await h.create("repair", budget);
    expect(result.details.reason).toBe("budget_unsupported");
    expect(h.control().status).toBe("cleared");
  }
  expect(h.messages).toHaveLength(0);
});

test("a restored legacy budgeted goal cannot resume", async () => {
  const sessions = new Map<string, Array<unknown>>([["session-A", [{
    type: "custom", customType: "pi-grok-goal/v1", data: { type: "goal_state", control: {
      sessionId: "session-A", goalId: "old", objective: "repair", status: "active", tokenBudget: 100,
    } },
  }]]]);
  const h = harness(sessions);
  await h.start();
  expect(h.control().status).toBe("user_paused");
  await h.command("resume");
  expect(h.notices.at(-1)).toContain("not enforced");
  expect((await h.resume()).details.reason).toBe("budget_unsupported");
  expect(h.control().status).toBe("user_paused");
  await h.command("clear");
  await h.command("repair without budget");
  expect(h.control().status).toBe("active");
});

test("model can resume a paused goal on explicit request", async () => {
  const h = harness();
  await h.start();
  await h.command("repair cabinet");
  const id = h.control().goalId;
  await h.command("pause");
  await h.resume();
  expect(h.control().status).toBe("active");
  expect(h.control().goalId).toBe(id);
});

test("model resume preserves safety boundaries and durable state", async () => {
  const sessions = new Map<string, Array<unknown>>();
  const h = harness(sessions);
  await h.start();
  expect((await h.resume()).details.reason).toBe("inactive");
  await h.command("repair");
  expect((await h.resume()).details.reason).toBe("active");
  await h.update({ blocked_reason: "missing dependency" });
  expect(h.control().status).toBe("blocked");
  expect((await h.update({ completed: true })).details.ok).toBe(false);
  expect((await h.resume()).details.ok).toBe(true);
  expect(h.control().pauseMessage).toBeUndefined();
  expect(latestGoalFromEntries(sessions.get("session-A")!, "session-A")?.status).toBe("active");
  const id = h.control().goalId;
  await h.command("clear");
  expect((await h.resume()).details.ok).toBe(true);
  expect(h.control().goalId).not.toBe(id);
  expect(h.control().objective).toBe("repair");
  await h.update({ completed: true });
  expect((await h.resume()).details.reason).toBe("inactive");
  expect(h.control().status).toBe("complete");
  h.switchTo("session-B");
  await h.start("new");
  expect((await h.resume()).details.reason).toBe("inactive");
  expect(h.control().sessionId).toBe("session-B");
});

test("model resume after restart does not queue another turn", async () => {
  const sessions = new Map<string, Array<unknown>>();
  const a = harness(sessions);
  await a.start();
  await a.command("restart work");
  const b = harness(sessions);
  await b.start();
  expect(b.control().status).toBe("user_paused");
  expect(b.messages).toHaveLength(0);
  expect((await b.resume()).details.ok).toBe(true);
  expect(b.control().status).toBe("active");
  expect(b.messages.at(-1)?.options.triggerTurn).toBe(false);
});

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

test("one goal tool exposes create/get/update/resume and rejects unknown actions", async () => {
  const h = harness();
  expect(h.toolNames()).toEqual(["goal"]);
  const definition = h.toolDefinition();
  for (const action of ["create", "get", "update", "resume"]) {
    expect(definition.description).toContain(action);
  }
  expect(definition.promptGuidelines).toHaveLength(3);
  await h.start();
  for (const action of [undefined, "delete", "pause"]) {
    const result = await h.goal(action ? { action } : {});
    expect(result.details.reason).toBe("invalid_action");
  }
  expect((await h.create("repair")).details.status).toBe("active");
  expect((await h.get()).details.goal.objective).toBe("repair");
  await h.command("pause");
  expect((await h.resume()).details.status).toBe("active");
  expect((await h.update({ message: "progress" })).details.status).toBe("active");
});

test("Both slash and goal create reject overwriting paused and blocked goals", async () => {
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

test("startup and tree navigation restore the selected branch only", async () => {
  const h = harness();
  await h.start();
  await h.command("branch A");
  const a = [...h.entries.get("session-A")!];
  await h.command("clear");
  await h.command("branch B");
  const b = [...h.entries.get("session-A")!];
  h.setBranch(a);
  await h.start();
  expect(h.control().objective).toBe("branch A");
  expect(h.control().status).toBe("user_paused");
  h.setBranch(b);
  await h.tree();
  expect(h.control().objective).toBe("branch B");
  expect(h.control().status).toBe("user_paused");
  h.setBranch([]);
  await h.tree();
  expect(h.control().status).toBe("cleared");
});

test("tree navigation publishes the selected state after writing it", async () => {
  const h = harness();
  await h.start();
  for (const status of ["active", "user_paused", "complete", "blocked", "cleared", "missing"]) {
    const control = { sessionId: "session-A", goalId: "historical", objective: "old goal", status, phase: "idle" };
    h.setBranch(status === "missing" ? [] : [{
      type: "custom", customType: "pi-grok-goal/v1", data: { type: "goal_state", control },
    }]);
    const previousCount = h.bridgeEvents.length;
    await h.tree();
    expect(h.bridgeEvents).toHaveLength(previousCount + 1);
    const { event, disk } = h.bridgeEvents.at(-1)!;
    expect(event.type).toBe("entry_appended");
    expect(event.entry.customType).toBe("pi-grok-goal/v1");
    expect(event.entry.data.event).toBe("goal_tree_restored");
    expect(event.entry.data.control).toEqual(disk);
    expect(disk.status).toBe(status === "active" ? "user_paused" : status === "missing" ? "cleared" : status);
  }
});

test("model context follows pause, clear, resume and cancellation without stale reminders", async () => {
  const h = harness();
  await h.start();
  expect((await h.get()).details.goal).toBeNull();
  await h.command("repair");
  await h.update({ message: "progress" });
  expect(h.messages.some((message) => message.options?.deliverAs === "nextTurn")).toBe(false);
  const stale = { role: "custom", customType: "pi-grok-goal/message/v1", content: "Goal mode is active." };
  await h.command("pause");
  let result = h.modelContext([stale]);
  expect(result.messages).toHaveLength(1);
  expect(result.messages[0].content).toContain("Status: user_paused");
  const paused = await h.get();
  expect(paused.content[0].text).toContain("Objective: repair");
  expect(paused.content[0].text).toContain("Status: user_paused");
  await h.command("clear");
  result = h.modelContext([stale]);
  expect(result.messages[0].content).toContain("Status: cleared");
  expect((await h.get()).details.goal.status).toBe("cleared");
  await h.resume();
  expect(h.modelContext([stale]).messages[0].content).toContain("Status: active");
  await h.settle(true);
  expect(h.modelContext([stale]).messages[0].content).toContain("Status: user_paused");
});

test("session_start journals an adapter loop guard before restoring the branch", async () => {
  const sessions = new Map<string, Array<unknown>>();
  const h = harness(sessions);
  await h.start();
  await h.command("repair");
  const goalId = h.control().goalId;
  h.writeControl({ ...h.control(), status: "blocked", phase: "idle", lastEvent: "goal_paused", lastEventDetail: "loop_guard" });
  await h.start("reload");
  expect(h.control().status).toBe("blocked");
  expect(h.control().lastEventDetail).toBe("loop_guard");
  expect(latestGoalFromEntries(sessions.get("session-A")!, "session-A")?.status).toBe("blocked");
  const restart = harness(sessions);
  await restart.start();
  expect(restart.control().status).toBe("blocked");
  expect(restart.control().goalId).toBe(goalId);
});

test("tree navigation never adopts an unscoped legacy goal", async () => {
  const legacy = {
    type: "custom", customType: "pi-grok-goal/v1",
    data: { type: "goal_state", control: {
      goalId: "legacy-parent", objective: "parent work", status: "active",
    } },
  };
  const h = harness();
  h.setBranch([legacy]);
  await h.tree();
  expect(h.control().status).toBe("cleared");
  expect(h.control().goalId).toBe("");
});

test("adapter loop guard is journaled and survives a restart", async () => {
  const sessions = new Map<string, Array<unknown>>();
  const h = harness(sessions);
  await h.start();
  await h.command("repair");
  const goalId = h.control().goalId;
  h.writeControl({ ...h.control(), status: "blocked", phase: "idle", lastEvent: "goal_paused", lastEventDetail: "loop_guard" });
  // The adapter invokes the hidden command with the goal it just blocked.
  await h.persist(goalId);
  expect(latestGoalFromEntries(sessions.get("session-A")!, "session-A")?.status).toBe("blocked");
  const entriesAfterFirst = sessions.get("session-A")!.length;
  await h.persist(goalId);
  expect(sessions.get("session-A")).toHaveLength(entriesAfterFirst);
  const restart = harness(sessions);
  await restart.start();
  expect(restart.control().status).toBe("blocked");
  expect((await restart.get()).details.goal.lastEventDetail).toBe("loop_guard");
});

test("loop guard persistence ignores a stale goal and recovers at lifecycle boundaries", async () => {
  const sessions = new Map<string, Array<unknown>>();
  const h = harness(sessions);
  await h.start();
  await h.command("repair");
  const goalId = h.control().goalId;
  h.writeControl({ ...h.control(), status: "blocked", phase: "idle", lastEvent: "goal_paused", lastEventDetail: "loop_guard" });
  const before = sessions.get("session-A")!.length;
  await h.persist("another-goal");
  expect(sessions.get("session-A")).toHaveLength(before);
  // A missed adapter command is recovered before the next model call.
  h.modelContext([]);
  expect(latestGoalFromEntries(sessions.get("session-A")!, "session-A")?.status).toBe("blocked");
});
