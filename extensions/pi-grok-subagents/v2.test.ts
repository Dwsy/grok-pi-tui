import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type {
  ExtensionAPI,
  ExtensionContext,
  ToolDefinition,
} from "@earendil-works/pi-coding-agent";
import type { SubagentRecord } from "./bridge.ts";
import type { RecordCreateOptions, SpawnParams } from "./runtime.ts";
import { configureSubagents } from "./config-ui.ts";
import { loadTeamDefinitions } from "./teams.ts";
import { TeamCoordinator, TEAM_MESSAGE_TYPE } from "./v2.ts";

class FakePi {
  readonly tools = new Map<string, ToolDefinition>();
  readonly commands = new Map<string, unknown>();
  readonly messages: Array<{ message: any; options: any }> = [];

  registerTool(tool: ToolDefinition): void {
    this.tools.set(tool.name, tool);
  }

  registerCommand(name: string, options: unknown): void {
    this.commands.set(name, options);
  }

  sendMessage(message: unknown, options: unknown): void {
    this.messages.push({ message, options });
  }
}

class FakeRuntime {
  readonly created: Array<{ record: SubagentRecord; options: RecordCreateOptions }> = [];
  readonly scheduled: string[] = [];
  readonly order: string[] = [];
  readonly childMessages = new Map<string, Array<{ message: any; options: any }>>();
  readonly finishHandlers = new Map<string, NonNullable<RecordCreateOptions["onFinished"]>>();
  readonly resumed: Array<{ previousId: string; nextId: string }> = [];
  readonly discarded: string[] = [];
  readonly backgroundStates = new Map<string, "running" | "queued" | "idle">();
  cancelled: string[] = [];
  failCreateAt = 0;
  private nextId = 1;

  async createRecord(
    _toolCallId: string,
    params: SpawnParams,
    _signal: AbortSignal | undefined,
    _ctx: ExtensionContext,
    options: RecordCreateOptions,
  ): Promise<SubagentRecord> {
    if (this.failCreateAt > 0 && this.created.length + 1 === this.failCreateAt) {
      throw new Error(`synthetic create failure ${this.failCreateAt}`);
    }
    const id = `fake-${this.nextId++}`;
    const delivered: Array<{ message: any; options: any }> = [];
    this.childMessages.set(id, delivered);
    const record = {
      id,
      prompt: params.prompt,
      description: params.description,
      finished: false,
      terminalStatus: null,
      lastError: undefined,
      session: {
        sendCustomMessage: async (message: unknown, sendOptions: unknown) => {
          delivered.push({ message, options: sendOptions });
        },
      },
    } as unknown as SubagentRecord;
    this.created.push({ record, options });
    this.order.push(`create:${params.description}`);
    if (options.onFinished) this.finishHandlers.set(id, options.onFinished);
    return record;
  }

  scheduleBackground(record: SubagentRecord): void {
    this.scheduled.push(record.description);
    this.order.push(`schedule:${record.description}`);
    this.backgroundStates.set(record.id, "running");
  }

  scheduleBackgroundTask(record: SubagentRecord, run: () => Promise<void>): "running" {
    this.scheduled.push(record.description);
    this.order.push(`schedule:${record.description}`);
    this.backgroundStates.set(record.id, "running");
    void run();
    return "running";
  }

  resumeRecord(
    previous: SubagentRecord,
    _toolCallId: string,
    prompt: string,
    _signal: AbortSignal | undefined,
    onFinished?: NonNullable<RecordCreateOptions["onFinished"]>,
  ): SubagentRecord {
    const nextId = `${previous.id}-resume-${this.resumed.length + 1}`;
    const record = {
      ...previous,
      id: nextId,
      prompt,
      finished: false,
      terminalStatus: null,
    } as SubagentRecord;
    this.childMessages.set(nextId, this.childMessages.get(previous.id) ?? []);
    this.backgroundStates.set(nextId, "idle");
    this.resumed.push({ previousId: previous.id, nextId });
    if (onFinished) this.finishHandlers.set(nextId, onFinished);
    return record;
  }

  async runCustomMessage(record: SubagentRecord, message: any, options: any): Promise<string> {
    this.childMessages.get(record.id)?.push({ message, options });
    record.finished = true;
    record.terminalStatus = "completed";
    const handler = this.finishHandlers.get(record.id);
    if (handler) await handler(record, "completed");
    return this.finalOutput(record);
  }

  finalOutput(record: SubagentRecord): string {
    return `final:${record.description}`;
  }

  cancel(record: SubagentRecord): void {
    this.cancelled.push(record.id);
  }

  discard(record: SubagentRecord): void {
    this.discarded.push(record.id);
    record.finished = true;
    record.terminalStatus = "cancelled";
  }

  backgroundState(record: SubagentRecord): "running" | "queued" | "idle" {
    return this.backgroundStates.get(record.id) ?? "running";
  }
}

const ctx = { cwd: process.cwd() } as ExtensionContext;

function setup() {
  const fakePi = new FakePi();
  const runtime = new FakeRuntime();
  const coordinator = new TeamCoordinator(fakePi as unknown as ExtensionAPI, runtime as any);
  coordinator.register();
  return { fakePi, runtime };
}

async function execute(
  tool: ToolDefinition | undefined,
  id: string,
  params: Record<string, unknown>,
) {
  assert.ok(tool, `missing tool ${id}`);
  return await tool.execute(id, params as never, undefined, undefined, ctx);
}

test("team discovery prefers project over global over bundled and isolates malformed files", () => {
  const root = mkdtempSync(join(tmpdir(), "pi-grok-teams-"));
  const projectDir = join(root, "project-config");
  const globalDir = join(root, "global-config");
  const previousProject = process.env.GROK_PROJECT_DIR;
  const previousGlobal = process.env.GROK_HOME;
  try {
    process.env.GROK_PROJECT_DIR = projectDir;
    process.env.GROK_HOME = globalDir;
    mkdirSync(join(projectDir, "teams"), { recursive: true });
    mkdirSync(join(globalDir, "teams"), { recursive: true });
    writeFileSync(join(globalDir, "teams", "research.json"), JSON.stringify({
      name: "research",
      description: "global research",
      members: [{ name: "global_worker", agent: "explore" }],
    }));
    writeFileSync(join(projectDir, "teams", "research.json"), JSON.stringify({
      name: "research",
      description: "project research",
      members: [{ name: "project_worker", agent: "plan" }],
    }));
    writeFileSync(join(projectDir, "teams", "review.json"), JSON.stringify({
      name: "review",
      enabled: false,
    }));
    writeFileSync(join(projectDir, "teams", "broken.json"), "{not-json");
    writeFileSync(join(projectDir, "teams", "implementation.json"), "{not-json");

    const teams = loadTeamDefinitions(process.cwd());
    assert.equal(teams.get("research")?.scope, "project");
    assert.equal(teams.get("research")?.description, "project research");
    assert.equal(teams.get("research")?.members[0]?.name, "project_worker");
    assert.equal(teams.get("review")?.scope, "project");
    assert.equal(teams.get("review")?.enabled, false);
    assert.equal(teams.has("broken"), false);
    assert.equal(teams.has("implementation"), false, "malformed project override must shadow bundled preset");
  } finally {
    if (previousProject === undefined) delete process.env.GROK_PROJECT_DIR;
    else process.env.GROK_PROJECT_DIR = previousProject;
    if (previousGlobal === undefined) delete process.env.GROK_HOME;
    else process.env.GROK_HOME = previousGlobal;
    rmSync(root, { recursive: true, force: true });
  }
});

test("bundled review preset triggers processing of late explorer evidence", () => {
  const review = JSON.parse(readFileSync(new URL("./teams/review.json", import.meta.url), "utf8")) as {
    members?: Array<{ name?: string; task?: string }>;
  };
  const explorer = review.members?.find((member) => member.name === "explorer");
  assert.ok(explorer?.task);
  assert.match(explorer.task, /team_followup_task/);
  assert.doesNotMatch(explorer.task, /team_send_message/);
});

test("V2 routes queue-only and triggering messages across root and child sessions", async () => {
  const { fakePi, runtime } = setup();
  const spawn = fakePi.tools.get("spawn_team_agent");
  const spawned = await execute(spawn, "spawn", {
    task_name: "worker",
    message: "work",
    agent_type: "explore",
  });
  assert.equal((spawned.details as any).agentPath, "/root/worker");
  assert.equal(runtime.scheduled.length, 1);

  const send = fakePi.tools.get("team_send_message");
  await execute(send, "send", { target: "/root/worker", message: "heads up" });
  const childDelivery = runtime.childMessages.get("fake-1")?.at(-1);
  assert.equal(childDelivery?.options.triggerTurn, false);
  assert.equal(childDelivery?.options.deliverAs, "steer");
  assert.equal(childDelivery?.message.customType, TEAM_MESSAGE_TYPE);

  const followup = fakePi.tools.get("team_followup_task");
  await execute(followup, "followup", { target: "/root/worker", message: "do another pass" });
  const followupDelivery = runtime.childMessages.get("fake-1")?.at(-1);
  assert.equal(followupDelivery?.options.triggerTurn, true);
  assert.equal(followupDelivery?.options.deliverAs, "followUp");

  const childTools = runtime.created[0].options.customTools ?? [];
  const childSend = childTools.find((tool) => tool.name === "team_send_message");
  await execute(childSend, "child-send", { target: "/root", message: "child report" });
  const rootDelivery = fakePi.messages.at(-1);
  assert.equal(rootDelivery?.options.triggerTurn, false);
  assert.equal(rootDelivery?.message.details.sender, "/root/worker");
  assert.equal(rootDelivery?.message.details.target, "/root");

  const nestedSpawn = childTools.find((tool) => tool.name === "spawn_team_agent");
  const nested = await execute(nestedSpawn, "nested", { task_name: "helper", message: "assist" });
  assert.equal((nested.details as any).agentPath, "/root/worker/helper");

  const finish = runtime.finishHandlers.get("fake-1");
  assert.ok(finish);
  runtime.created[0].record.finished = true;
  runtime.created[0].record.terminalStatus = "completed";
  await finish(runtime.created[0].record, "completed");
  const finalDelivery = fakePi.messages.at(-1);
  assert.equal(finalDelivery?.options.triggerTurn, true);
  assert.equal(finalDelivery?.message.details.kind, "FINAL_ANSWER");
});

test("completed team agents become idle and can be reactivated in the same child session", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", {
    task_name: "worker",
    message: "first task",
  });
  const record = runtime.created[0].record;
  const finish = runtime.finishHandlers.get(record.id);
  assert.ok(finish);
  record.finished = true;
  record.terminalStatus = "completed";
  await finish(record, "completed");

  const listed = await execute(fakePi.tools.get("team_list"), "list", {});
  const listedText = listed.content[0].type === "text" ? listed.content[0].text : "";
  assert.match(listedText, /\[IDLE\] \/root\/worker/);

  const followup = await execute(fakePi.tools.get("team_followup_task"), "followup-idle", {
    target: "/root/worker",
    message: "second task",
  });
  assert.equal((followup.details as any).reactivated, true);
  assert.equal(runtime.resumed.length, 1);
  assert.equal(runtime.resumed[0].previousId, record.id);
  assert.notEqual(runtime.resumed[0].nextId, record.id, "each reactivation needs a fresh V1 lifecycle id");
  assert.equal(runtime.created.length, 1, "reactivation must reuse the existing child session");
  const delivery = runtime.childMessages.get(runtime.resumed[0].nextId)?.at(-1);
  assert.equal(delivery?.message.details.kind, "NEW_TASK");
  assert.equal(delivery?.options.triggerTurn, true);
});

test("follow-up for an agent waiting in the concurrency queue is deferred until its current run finishes", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "first" });
  const record = runtime.created[0].record;
  runtime.backgroundStates.set(record.id, "queued");

  const queued = await execute(fakePi.tools.get("team_followup_task"), "queued-followup", {
    target: "/root/worker",
    message: "second",
  });
  assert.equal((queued.details as any).queued, true);
  assert.equal(runtime.resumed.length, 0);

  runtime.backgroundStates.set(record.id, "idle");
  record.finished = true;
  record.terminalStatus = "completed";
  const finish = runtime.finishHandlers.get(record.id);
  assert.ok(finish);
  await finish(record, "completed");
  assert.equal(runtime.resumed[0]?.previousId, record.id);
  const delivery = runtime.childMessages.get(runtime.resumed[0].nextId)?.at(-1);
  assert.equal(delivery?.message.details.kind, "NEW_TASK");
});

test("nested FINAL_ANSWER reactivates an idle parent instead of dropping the result", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "parent", { task_name: "parent", message: "parent work" });
  const parent = runtime.created[0];
  const childSpawn = (parent.options.customTools ?? []).find((tool) => tool.name === "spawn_team_agent");
  await execute(childSpawn, "child", { task_name: "child", message: "child work" });
  const child = runtime.created[1];

  runtime.backgroundStates.set(parent.record.id, "idle");
  parent.record.finished = true;
  parent.record.terminalStatus = "completed";
  const parentFinish = runtime.finishHandlers.get(parent.record.id);
  assert.ok(parentFinish);
  await parentFinish(parent.record, "completed");

  runtime.backgroundStates.set(child.record.id, "idle");
  child.record.finished = true;
  child.record.terminalStatus = "completed";
  const childFinish = runtime.finishHandlers.get(child.record.id);
  assert.ok(childFinish);
  await childFinish(child.record, "completed");

  const parentResume = runtime.resumed.find((entry) => entry.previousId === parent.record.id);
  assert.ok(parentResume);
  const parentDelivery = runtime.childMessages.get(parentResume.nextId)?.find((entry) => entry.message.details.kind === "FINAL_ANSWER");
  assert.ok(parentDelivery, "idle parent must receive nested child FINAL_ANSWER");
  assert.equal(parentDelivery?.message.details.sender, "/root/parent/child");
});

test("nested FINAL_ANSWER waits behind a queued parent instead of bypassing the scheduler", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "parent", { task_name: "parent", message: "parent work" });
  const parent = runtime.created[0];
  const childSpawn = (parent.options.customTools ?? []).find((tool) => tool.name === "spawn_team_agent");
  await execute(childSpawn, "child", { task_name: "child", message: "child work" });
  const child = runtime.created[1];

  runtime.backgroundStates.set(parent.record.id, "queued");
  child.record.finished = true;
  child.record.terminalStatus = "completed";
  const childFinish = runtime.finishHandlers.get(child.record.id);
  assert.ok(childFinish);
  await childFinish(child.record, "completed");
  assert.equal(runtime.resumed.length, 0, "queued parent must not be reactivated early");
  assert.equal(
    runtime.childMessages.get(parent.record.id)?.some((entry) => entry.message.details?.kind === "FINAL_ANSWER"),
    false,
    "queued parent must not receive a direct triggering message",
  );

  runtime.backgroundStates.set(parent.record.id, "idle");
  parent.record.finished = true;
  parent.record.terminalStatus = "completed";
  const parentFinish = runtime.finishHandlers.get(parent.record.id);
  assert.ok(parentFinish);
  await parentFinish(parent.record, "completed");

  const parentResume = runtime.resumed.find((entry) => entry.previousId === parent.record.id);
  assert.ok(parentResume);
  const delivery = runtime.childMessages.get(parentResume.nextId)?.find((entry) => entry.message.details?.kind === "FINAL_ANSWER");
  assert.ok(delivery, "deferred FINAL_ANSWER must reactivate the parent only after its queued run finishes");
});

test("spawn_team rolls back already-created members when a later member fails", async () => {
  const { fakePi, runtime } = setup();
  runtime.failCreateAt = 2;
  await assert.rejects(
    execute(fakePi.tools.get("spawn_team"), "team-fail", { team: "research", task: "trace a bug" }),
    /synthetic create failure 2/,
  );
  assert.deepEqual(runtime.discarded, ["fake-1"]);
  assert.equal(runtime.scheduled.length, 0);
  const listed = await execute(fakePi.tools.get("team_list"), "list-after-fail", {});
  assert.equal((listed.details as any).agents.length, 0);
});

test("spawn_team registers the full roster before starting any member", async () => {
  const { fakePi, runtime } = setup();
  const spawnTeam = fakePi.tools.get("spawn_team");
  const result = await execute(spawnTeam, "team", { team: "research", task: "trace a bug" });
  assert.equal((result.details as any).agents.length, 3);
  assert.deepEqual(runtime.created.map((entry) => entry.record.description), [
    "Primary researcher",
    "Critical reviewer",
    "Research synthesizer",
  ]);
  assert.equal(runtime.scheduled.length, 3);
  const firstSchedule = runtime.order.findIndex((entry) => entry.startsWith("schedule:"));
  const lastCreate = runtime.order.map((entry, index) => [entry, index] as const)
    .filter(([entry]) => entry.startsWith("create:"))
    .at(-1)?.[1] ?? -1;
  assert.ok(firstSchedule > lastCreate, runtime.order.join(", "));
});

test("team_wait wakes on activity and team_interrupt delegates cancellation", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "work" });
  const waitPromise = execute(fakePi.tools.get("team_wait"), "wait", { timeout_ms: 5_000 });
  await execute(fakePi.tools.get("team_send_message"), "send", { target: "/root/worker", message: "wake" });
  const waited = await waitPromise;
  assert.equal((waited.details as any).activity, true);

  await execute(fakePi.tools.get("team_interrupt"), "interrupt", { target: "/root/worker" });
  assert.deepEqual(runtime.cancelled, ["fake-1"]);
});


test("subagent config selection matches exact labels instead of name prefixes", async () => {
  const root = mkdtempSync(join(tmpdir(), "pi-grok-agent-config-"));
  const previousProject = process.env.GROK_PROJECT_DIR;
  try {
    process.env.GROK_PROJECT_DIR = root;
    mkdirSync(join(root, "agents"), { recursive: true });
    const body = (name: string) => `---\ndescription: ${JSON.stringify(name)}\nenabled: true\n---\n\n${name}\n`;
    writeFileSync(join(root, "agents", "foo.md"), body("foo"));
    writeFileSync(join(root, "agents", "foo-bar.md"), body("foo-bar"));
    const titles: string[] = [];
    let selectCount = 0;
    const configCtx = {
      cwd: process.cwd(),
      ui: {
        select: async (title: string) => {
          titles.push(title);
          selectCount += 1;
          return selectCount === 1 ? "project: foo-bar" : undefined;
        },
        notify: () => undefined,
      },
    } as any;
    await configureSubagents({} as ExtensionAPI, configCtx);
    assert.equal(titles[1], "Subagent foo-bar (project)");
  } finally {
    if (previousProject === undefined) delete process.env.GROK_PROJECT_DIR;
    else process.env.GROK_PROJECT_DIR = previousProject;
    rmSync(root, { recursive: true, force: true });
  }
});

test("cancelling a new subagent edit does not create a definition file", async () => {
  const root = mkdtempSync(join(tmpdir(), "pi-grok-agent-cancel-"));
  const previousProject = process.env.GROK_PROJECT_DIR;
  try {
    process.env.GROK_PROJECT_DIR = root;
    let selectCount = 0;
    const configCtx = {
      cwd: process.cwd(),
      ui: {
        select: async () => {
          selectCount += 1;
          return selectCount === 1 ? "New project subagent" : undefined;
        },
        input: async () => "cancel_me",
        notify: () => undefined,
      },
    } as any;
    await configureSubagents({} as ExtensionAPI, configCtx);
    assert.equal(existsSync(join(root, "agents", "cancel_me.md")), false);
  } finally {
    if (previousProject === undefined) delete process.env.GROK_PROJECT_DIR;
    else process.env.GROK_PROJECT_DIR = previousProject;
    rmSync(root, { recursive: true, force: true });
  }
});
