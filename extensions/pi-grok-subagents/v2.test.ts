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
  readonly created: Array<{ record: SubagentRecord; params: SpawnParams; options: RecordCreateOptions }> = [];
  readonly toolCallIds: string[] = [];
  readonly parentToolCallIds: Array<string | undefined> = [];
  readonly scheduled: string[] = [];
  readonly order: string[] = [];
  readonly childMessages = new Map<string, Array<{ message: any; options: any }>>();
  readonly finishHandlers = new Map<string, NonNullable<RecordCreateOptions["onFinished"]>>();
  readonly resumed: Array<{ previousId: string; nextId: string }> = [];
  readonly discarded: string[] = [];
  readonly backgroundStates = new Map<string, "running" | "queued" | "idle">();
  readonly promoted: string[] = [];
  cancelled: string[] = [];
  failCreateAt = 0;
  private nextId = 1;

  async createRecord(
    toolCallId: string,
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
      childSessionId: id,
      prompt: params.prompt,
      description: params.description,
      agentPath: options.teamIdentity?.agentPath,
      parentAgentPath: options.teamIdentity?.parentAgentPath,
      team: options.teamIdentity?.team,
      bridgeParentSessionId: options.teamIdentity?.bridgeParentSessionId,
      finished: false,
      terminalStatus: null,
      lastError: undefined,
      session: {
        sendCustomMessage: async (message: unknown, sendOptions: unknown) => {
          delivered.push({ message, options: sendOptions });
        },
      },
    } as unknown as SubagentRecord;
    this.created.push({ record, params, options });
    this.toolCallIds.push(toolCallId);
    this.parentToolCallIds.push(options.parentToolCallId);
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

  async waitForRecords(): Promise<void> {}

  formatDuration(ms: number): string {
    return `${Math.round(ms / 1000)}s`;
  }

  prioritizeBackground(record: SubagentRecord): boolean {
    if (this.backgroundStates.get(record.id) !== "queued") return false;
    this.promoted.push(record.id);
    return true;
  }

  formatSubagentResult(record: SubagentRecord): string {
    return `result:${record.description}`;
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
  signal?: AbortSignal,
) {
  assert.ok(tool, `missing tool ${id}`);
  return await tool.execute(id, params as never, signal, undefined, ctx);
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
  const buffered = await execute(send, "send", { target: "/root/worker", message: "heads up" });
  assert.equal((buffered.details as any).buffered, 1);
  assert.equal(runtime.childMessages.get("fake-1")?.length, 0, "a buffered message waits for the next team action");

  const followup = fakePi.tools.get("team_followup_task");
  await execute(followup, "followup", { target: "/root/worker", message: "do another pass" });
  const childDelivery = (runtime.childMessages.get("fake-1") ?? [])
    .find((entry) => entry.message.details.kind === "MESSAGE");
  assert.equal(childDelivery?.options.triggerTurn, false);
  assert.equal(childDelivery?.options.deliverAs, "steer");
  assert.equal(childDelivery?.message.customType, TEAM_MESSAGE_TYPE);
  const followupDelivery = runtime.childMessages.get("fake-1")?.at(-1);
  assert.equal(followupDelivery?.options.triggerTurn, true);
  assert.equal(followupDelivery?.options.deliverAs, "followUp");

  const childTools = runtime.created[0].options.customTools ?? [];
  const childSend = childTools.find((tool) => tool.name === "team_send_message");
  await execute(childSend, "child-send", { target: "/root", message: "child report" });
  const childList = childTools.find((tool) => tool.name === "team_list");
  await execute(childList, "child-list", {});
  const rootDelivery = fakePi.messages.at(-1);
  assert.equal(rootDelivery?.options.triggerTurn, false);
  assert.equal(rootDelivery?.message.details.sender, "/root/worker");
  assert.equal(rootDelivery?.message.details.target, "/root");

  const nestedSpawn = childTools.find((tool) => tool.name === "spawn_team_agent");
  const nested = await execute(nestedSpawn, "nested", { task_name: "helper", message: "assist" });
  assert.equal((nested.details as any).agentPath, "/root/worker/helper");
  assert.deepEqual(runtime.created.map(({ record }) => [record.agentPath, record.parentAgentPath]), [
    ["/root/worker", "/root"],
    ["/root/worker/helper", "/root/worker"],
  ]);
  assert.equal(runtime.created[0].record.bridgeParentSessionId, undefined);
  assert.equal(runtime.created[1].record.bridgeParentSessionId, "fake-1");

  const finish = runtime.finishHandlers.get("fake-1");
  assert.ok(finish);
  runtime.created[0].record.finished = true;
  runtime.created[0].record.terminalStatus = "completed";
  await finish(runtime.created[0].record, "completed");
  const finalDelivery = fakePi.messages.at(-1);
  assert.equal(finalDelivery?.options.triggerTurn, true);
  assert.equal(finalDelivery?.message.details.kind, "FINAL_ANSWER");
});

test("reactivating an agent clears the previous report", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "first" });
  const record = runtime.created[0].record;
  const childReport = (runtime.created[0].options.customTools ?? []).find((tool) => tool.name === "team_report");
  await execute(childReport, "report", { state: "dropped", summary: "Nothing to do." });
  const finish = runtime.finishHandlers.get(record.id);
  assert.ok(finish);
  record.finished = true;
  record.terminalStatus = "completed";
  await finish(record, "completed");
  assert.equal(fakePi.messages.length, 0, "a dropped run stays silent");

  await execute(fakePi.tools.get("team_followup_task"), "followup", {
    target: "/root/worker",
    message: "do it now",
  });
  await new Promise<void>((resolve) => setImmediate(resolve));

  assert.equal(fakePi.messages.length, 1, "the new run must deliver despite the old dropped report");
  assert.match((fakePi.messages[0].message as any).content, /final:worker/);
});

test("a reactivated blocked agent does not repeat its old question", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "pick a design" });
  const record = runtime.created[0].record;
  const childReport = (runtime.created[0].options.customTools ?? []).find((tool) => tool.name === "team_report");
  await execute(childReport, "report", {
    state: "blocked",
    summary: "Both pass.",
    question: "Streaming or buffered?",
  });
  const finish = runtime.finishHandlers.get(record.id);
  assert.ok(finish);
  record.finished = true;
  record.terminalStatus = "completed";
  await finish(record, "completed");
  assert.match((fakePi.messages.at(-1)?.message as any).content, /Streaming or buffered\?/);

  await execute(fakePi.tools.get("team_followup_task"), "followup", {
    target: "/root/worker",
    message: "Use streaming.",
  });
  await new Promise<void>((resolve) => setImmediate(resolve));

  const delivered = (fakePi.messages.at(-1)?.message as any).content as string;
  assert.doesNotMatch(delivered, /Streaming or buffered\?/, "the old question must not be replayed");
  assert.match(delivered, /final:worker/);
});

test("consecutive team_send_message calls merge into one steer", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "work" });
  const send = fakePi.tools.get("team_send_message");
  await execute(send, "send-1", { target: "/root/worker", message: "found the parser" });
  await execute(send, "send-2", { target: "/root/worker", message: "tests fail on empty input" });
  await execute(send, "send-3", { target: "/root/worker", message: "found the parser" });
  await execute(fakePi.tools.get("team_list"), "flush", {});

  const delivered = (runtime.childMessages.get("fake-1") ?? [])
    .filter((entry) => entry.message.details.kind === "MESSAGE");
  assert.equal(delivered.length, 1, "one merged steer instead of three");
  const content = delivered[0]?.message.content ?? "";
  assert.match(content, /1\. found the parser/);
  assert.match(content, /2\. tests fail on empty input/);
  assert.equal((content.match(/found the parser/g) ?? []).length, 1, "repeated text is not duplicated");
});

test("team_list reports the quiet gap for each agent", async () => {
  const { fakePi } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "work" });
  const listed = await execute(fakePi.tools.get("team_list"), "list", {});
  assert.match((listed.content as any)[0].text, /quiet \d+s/);
  const agent = (listed.details as any).agents[0];
  assert.equal(typeof agent.quietMs, "number");
});

test("a blocked report keeps the agent waiting on a decision and still reachable", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "pick a design" });
  const record = runtime.created[0].record;
  const childReport = (runtime.created[0].options.customTools ?? []).find((tool) => tool.name === "team_report");
  await execute(childReport, "report", {
    state: "blocked",
    summary: "Both designs pass.",
    question: "Streaming or buffered?",
  });
  const finish = runtime.finishHandlers.get(record.id);
  assert.ok(finish);
  record.finished = true;
  record.terminalStatus = "completed";
  await finish(record, "completed");

  const listed = await execute(fakePi.tools.get("team_list"), "list", {});
  assert.match((listed.content as any)[0].text, /\[WAITING_DECISION\] \/root\/worker/);
  assert.match((listed.content as any)[0].text, /Streaming or buffered\?/);

  const send = fakePi.tools.get("team_send_message");
  await execute(send, "answer", { target: "/root/worker", message: "Use streaming." });
  await execute(fakePi.tools.get("team_list"), "flush", {});
  const delivered = (runtime.childMessages.get("fake-1") ?? [])
    .find((entry) => entry.message.details.kind === "MESSAGE");
  assert.match(delivered?.message.content ?? "", /Use streaming\./);
});

test("team_followup_task promotes a queued agent ahead of unrelated launches", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "first" });
  const record = runtime.created[0].record;
  runtime.backgroundStates.set(record.id, "queued");

  const queued = await execute(fakePi.tools.get("team_followup_task"), "queued-followup", {
    target: "/root/worker",
    message: "urgent",
  });
  assert.equal((queued.details as any).queued, true);
  assert.equal((queued.details as any).promoted, true);
  assert.deepEqual(runtime.promoted, [record.id]);
});

test("team_report dropped keeps the parent silent", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "check it" });
  const record = runtime.created[0].record;
  const childReport = (runtime.created[0].options.customTools ?? []).find((tool) => tool.name === "team_report");
  assert.ok(childReport, "children must receive team_report");
  await execute(childReport, "report-dropped", {
    state: "dropped",
    summary: "Nothing to change; already fixed upstream.",
  });

  const finish = runtime.finishHandlers.get(record.id);
  assert.ok(finish);
  record.finished = true;
  record.terminalStatus = "completed";
  await finish(record, "completed");

  assert.equal(fakePi.messages.length, 0, "a dropped result must not wake the parent");
  const listed = await execute(fakePi.tools.get("team_list"), "list", {});
  assert.match((listed.content as any)[0].text, /dropped — Nothing to change/);
});

test("waiting report yields a coordinator and nested results resume its session", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "parent", { task_name: "parent", message: "coordinate" });
  const parent = runtime.created[0];
  const tools = parent.options.customTools ?? [];
  await execute(tools.find((tool) => tool.name === "spawn_team_agent"), "leaf", { task_name: "leaf", message: "verify" });
  const leaf = runtime.created[1];
  runtime.backgroundStates.set(leaf.record.id, "queued");
  const reported = await execute(tools.find((tool) => tool.name === "team_report"), "waiting", {
    state: "waiting",
    summary: "Waiting for leaf verification.",
  });
  assert.match((reported.content as any)[0].text, /End this turn to release/);
  assert.equal(parent.record.finished, false, "reporting alone must not finish the run");

  parent.record.finished = true;
  parent.record.terminalStatus = "completed";
  await runtime.finishHandlers.get(parent.record.id)!(parent.record, "completed");
  assert.equal(fakePi.messages.length, 0, "partial coordination must not wake root");
  const listed = await execute(fakePi.tools.get("team_list"), "list", {});
  assert.match((listed.content as any)[0].text, /\[WAITING_RESULT\] \/root\/parent/);

  leaf.record.finished = true;
  leaf.record.terminalStatus = "completed";
  await runtime.finishHandlers.get(leaf.record.id)!(leaf.record, "completed");
  const resumed = runtime.resumed.find((entry) => entry.previousId === parent.record.id);
  assert.ok(resumed, "a result must resume the waiting coordinator");
  const delivered = runtime.childMessages.get(resumed.nextId)?.find((entry) => entry.message.details.kind === "FINAL_ANSWER");
  assert.equal(delivered?.message.details.sender, "/root/parent/leaf");
  assert.equal(fakePi.messages.length, 1, "the resumed run must not inherit a stale waiting report");
  assert.match((fakePi.messages[0].message as any).content, /final:parent/);
});

test("waiting rejects unrelated active peers and a child waiting on a decision", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "parent", { task_name: "parent", message: "coordinate" });
  await execute(fakePi.tools.get("spawn_team_agent"), "peer", { task_name: "peer", message: "unrelated" });
  const tools = runtime.created[0].options.customTools ?? [];
  const report = tools.find((tool) => tool.name === "team_report");
  await assert.rejects(execute(report, "no-child", { state: "waiting", summary: "Waiting." }), /requires your own pending child/);

  await execute(tools.find((tool) => tool.name === "spawn_team_agent"), "leaf", { task_name: "leaf", message: "decide" });
  const leaf = runtime.created[2];
  await execute(leaf.options.customTools?.find((tool) => tool.name === "team_report"), "blocked", {
    state: "blocked", summary: "Need a choice.", question: "Which API?",
  });
  leaf.record.finished = true;
  leaf.record.terminalStatus = "completed";
  await assert.rejects(execute(report, "blocked-child", { state: "waiting", summary: "Waiting." }), /requires your own pending child/);
});

test("waiting can propagate through multiple non-running coordinators", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "parent", { task_name: "parent", message: "coordinate" });
  const parentTools = runtime.created[0].options.customTools ?? [];
  await execute(parentTools.find((tool) => tool.name === "spawn_team_agent"), "middle", { task_name: "middle", message: "coordinate" });
  const middle = runtime.created[1];
  const middleTools = middle.options.customTools ?? [];
  await execute(middleTools.find((tool) => tool.name === "spawn_team_agent"), "leaf", { task_name: "leaf", message: "work" });
  await execute(middleTools.find((tool) => tool.name === "team_report"), "middle-waiting", { state: "waiting", summary: "Leaf pending." });
  middle.record.finished = true;
  middle.record.terminalStatus = "completed";
  await runtime.finishHandlers.get(middle.record.id)!(middle.record, "completed");
  await execute(parentTools.find((tool) => tool.name === "team_report"), "parent-waiting", { state: "waiting", summary: "Middle pending." });
  assert.equal(fakePi.messages.length, 0);
});

test("quiet reports cannot hide runtime failure or cancellation", async () => {
  for (const state of ["waiting", "dropped"] as const) {
    for (const terminal of ["failed", "cancelled"] as const) {
      const { fakePi, runtime } = setup();
      await execute(fakePi.tools.get("spawn_team_agent"), "parent", { task_name: "parent", message: "work" });
      const parent = runtime.created[0];
      const tools = parent.options.customTools ?? [];
      if (state === "waiting") {
        await execute(tools.find((tool) => tool.name === "spawn_team_agent"), "leaf", { task_name: "leaf", message: "work" });
      }
      await execute(tools.find((tool) => tool.name === "team_report"), "report", { state, summary: "Quiet result." });
      parent.record.finished = true;
      parent.record.terminalStatus = terminal;
      await runtime.finishHandlers.get(parent.record.id)!(parent.record, terminal, "synthetic failure");
      assert.equal(fakePi.messages.length, 1, `${state} must not suppress ${terminal}`);
      assert.match((fakePi.messages[0].message as any).content, new RegExp(`Agent ${terminal}`));
    }
  }
});

test("a dropped child still resolves a waiting coordinator", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "parent", { task_name: "parent", message: "coordinate" });
  const parent = runtime.created[0];
  const parentTools = parent.options.customTools ?? [];
  await execute(parentTools.find((tool) => tool.name === "spawn_team_agent"), "leaf", { task_name: "leaf", message: "check" });
  const leaf = runtime.created[1];
  await execute(parentTools.find((tool) => tool.name === "team_report"), "waiting", { state: "waiting", summary: "Check pending." });
  parent.record.finished = true;
  parent.record.terminalStatus = "completed";
  await runtime.finishHandlers.get(parent.record.id)!(parent.record, "completed");
  await execute(leaf.options.customTools?.find((tool) => tool.name === "team_report"), "dropped", { state: "dropped", summary: "Already satisfied." });
  leaf.record.finished = true;
  leaf.record.terminalStatus = "completed";
  await runtime.finishHandlers.get(leaf.record.id)!(leaf.record, "completed");
  const resumed = runtime.resumed.find((entry) => entry.previousId === parent.record.id);
  assert.ok(resumed, "a no-op result must resolve the parent's pending dependency");
  const delivery = runtime.childMessages.get(resumed.nextId)?.find((entry) => entry.message.details.kind === "FINAL_ANSWER");
  assert.match(delivery?.message.content ?? "", /Report: dropped/);
});

test("a dropped result arriving before the coordinator ends its turn is delivered", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "parent", { task_name: "parent", message: "coordinate" });
  const parent = runtime.created[0];
  const tools = parent.options.customTools ?? [];
  await execute(tools.find((tool) => tool.name === "spawn_team_agent"), "leaf", { task_name: "leaf", message: "check" });
  const leaf = runtime.created[1];
  await execute(tools.find((tool) => tool.name === "team_report"), "waiting", { state: "waiting", summary: "Check pending." });
  await execute(leaf.options.customTools?.find((tool) => tool.name === "team_report"), "dropped", { state: "dropped", summary: "Already satisfied." });
  leaf.record.finished = true;
  leaf.record.terminalStatus = "completed";
  await runtime.finishHandlers.get(leaf.record.id)!(leaf.record, "completed");
  assert.equal(runtime.resumed.length, 0, "a running coordinator must not get a second run");
  const delivery = runtime.childMessages.get(parent.record.id)?.find((entry) => entry.message.details.kind === "FINAL_ANSWER");
  assert.match(delivery?.message.content ?? "", /Report: dropped/);
  assert.equal(delivery?.options.triggerTurn, true);
  assert.equal(fakePi.messages.length, 0, "the intermediate result stays at the coordinator");
});

test("team_report blocked carries the parent decision into the final answer", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "pick a design" });
  const record = runtime.created[0].record;
  const childReport = (runtime.created[0].options.customTools ?? []).find((tool) => tool.name === "team_report");
  await execute(childReport, "report-blocked", {
    state: "blocked",
    summary: "Both designs pass the checks.",
    question: "Use the streaming parser or the buffered one?",
  });

  const finish = runtime.finishHandlers.get(record.id);
  assert.ok(finish);
  record.finished = true;
  record.terminalStatus = "completed";
  await finish(record, "completed");

  const delivered = fakePi.messages.at(-1);
  const message = delivered?.message as any;
  assert.equal(message.details.kind, "FINAL_ANSWER");
  assert.match(message.content, /Report: blocked/);
  assert.match(message.content, /Needs your decision: Use the streaming parser or the buffered one\?/);
  assert.equal(delivered?.options.triggerTurn, true);
});

test("team_report rejects blocked without a question", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "work" });
  const childReport = (runtime.created[0].options.customTools ?? []).find((tool) => tool.name === "team_report");
  await assert.rejects(
    execute(childReport, "report-blocked-bad", { state: "blocked", summary: "Stuck." }),
    /requires the question/,
  );
});

test("a child without team_report keeps the unconditional final answer", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "work" });
  const record = runtime.created[0].record;
  const finish = runtime.finishHandlers.get(record.id);
  assert.ok(finish);
  record.finished = true;
  record.terminalStatus = "completed";
  await finish(record, "completed");

  const delivered = fakePi.messages.at(-1);
  assert.match((delivered?.message as any).content, /final:worker/);
  assert.equal(delivered?.options.triggerTurn, true);
});

test("root has no team_report because it has no parent", () => {
  const { fakePi, runtime } = setup();
  assert.equal(fakePi.tools.has("team_report"), false);
  assert.equal(runtime.created.length, 0);
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

test("spawn_team starts only the requested member subset in preset order", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team"), "team-subset", {
    team: "research",
    task: "trace a bug",
    members: ["synthesizer", "researcher"],
  });
  assert.deepEqual(runtime.created.map((entry) => entry.record.description), [
    "Primary researcher",
    "Research synthesizer",
  ]);
});

test("spawn_team rejects member names that the preset does not define", async () => {
  const { fakePi, runtime } = setup();
  await assert.rejects(
    execute(fakePi.tools.get("spawn_team"), "team-unknown", {
      team: "research",
      task: "trace a bug",
      members: ["ghost"],
    }),
    /has no member named ghost/,
  );
  assert.equal(runtime.created.length, 0);
});

test("spawn_team applies call-level model, thinking, turn, and capability overrides", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team"), "team-override", {
    team: "research",
    task: "trace a bug",
    members: ["researcher"],
    model: "anthropic/claude-sonnet-4",
    thinking_level: "high",
    max_turns: 7,
    capability_mode: "read-only",
    instructions: "Report only verifiable evidence.",
  });
  const entry = runtime.created[0];
  assert.equal(entry.params.model, "anthropic/claude-sonnet-4");
  assert.equal(entry.params.thinking_level, "high");
  assert.equal(entry.params.max_turns, 7);
  assert.equal(entry.params.capability_mode, "read-only");
  assert.match(entry.options.systemPromptSuffix ?? "", /Report only verifiable evidence\./);
});

test("spawn_team omits the single-parent task id for a one-to-many spawn", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team"), "call-real", { team: "research", task: "trace a bug" });
  assert.deepEqual(runtime.toolCallIds, ["call-real", "call-real", "call-real"]);
  assert.deepEqual(runtime.parentToolCallIds, ["", "", ""]);
});

test("spawn_team discards every created member when cancelled before start", async () => {
  const { fakePi, runtime } = setup();
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(
    execute(fakePi.tools.get("spawn_team"), "team-abort", { team: "research", task: "trace a bug" }, controller.signal),
    /cancelled before any member started/,
  );
  assert.equal(runtime.scheduled.length, 0);
  assert.deepEqual(runtime.discarded, ["fake-1", "fake-2", "fake-3"]);
});

test("spawn_team returns member results when timeout_ms is positive", async () => {
  const { fakePi } = setup();
  const result = await execute(fakePi.tools.get("spawn_team"), "team-wait", {
    team: "research",
    task: "trace a bug",
    members: ["critic"],
    timeout_ms: 1_000,
  });
  assert.match((result.content as any)[0].text, /result:Critical reviewer/);
});

test("spawn_team_agent forwards capability_mode and description", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "agent", {
    task_name: "worker",
    message: "work",
    capability_mode: "read-only",
    description: "Read-only auditor",
  });
  assert.equal(runtime.created[0].params.capability_mode, "read-only");
  assert.equal(runtime.created[0].record.description, "Read-only auditor");
});

test("team_list shows which preset owns each agent", async () => {
  const { fakePi } = setup();
  await execute(fakePi.tools.get("spawn_team"), "team", {
    team: "research",
    task: "trace a bug",
    members: ["critic"],
  });
  const listed = await execute(fakePi.tools.get("team_list"), "list", {});
  assert.match((listed.content as any)[0].text, /\/root\/critic \(plan\) \[research\]/);
});

test("team_wait returns immediately when a child has no active peers", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "work" });
  const childWait = (runtime.created[0].options.customTools ?? []).find((tool) => tool.name === "team_wait");

  const startedAt = Date.now();
  const waited = await execute(childWait, "child-wait-idle", { timeout_ms: 1_000 });
  const elapsedMs = Date.now() - startedAt;

  assert.equal((waited.details as any).activity, false);
  assert.equal((waited.details as any).idle, true);
  assert.ok(elapsedMs < 250, `idle team_wait blocked for ${elapsedMs}ms`);
});

test("root team_wait returns immediately while background agents run", async () => {
  const { fakePi } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn", { task_name: "worker", message: "work" });

  const startedAt = Date.now();
  const waited = await execute(fakePi.tools.get("team_wait"), "root-wait", { timeout_ms: 5_000 });
  const elapsedMs = Date.now() - startedAt;

  assert.equal((waited.details as any).activity, false);
  assert.equal((waited.details as any).idle, true);
  assert.equal((waited.details as any).root, true);
  assert.ok(elapsedMs < 250, `root team_wait blocked for ${elapsedMs}ms`);
});

test("child team_wait wakes on activity and team_interrupt delegates cancellation", async () => {
  const { fakePi, runtime } = setup();
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn-worker", { task_name: "worker", message: "work" });
  await execute(fakePi.tools.get("spawn_team_agent"), "spawn-peer", { task_name: "peer", message: "peer work" });
  const childWait = (runtime.created[0].options.customTools ?? []).find((tool) => tool.name === "team_wait");
  const waitPromise = execute(childWait, "child-wait", { timeout_ms: 5_000 });
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
