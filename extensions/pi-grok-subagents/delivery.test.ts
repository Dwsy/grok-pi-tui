import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { Type } from "typebox";
import { createAssistantMessageEventStream, type AssistantMessage } from "@earendil-works/pi-ai";
import {
  createAgentSession, DefaultResourceLoader, defineTool, ModelRuntime, SessionManager, SettingsManager,
  type AgentSession, type ExtensionAPI, type ExtensionContext, type ToolDefinition,
} from "@earendil-works/pi-coding-agent";
import type { SubagentRecord } from "./bridge.ts";
import type { RecordCreateOptions, RecordFinishHandler, SpawnParams } from "./runtime.ts";
import { SubagentRuntime } from "./runtime.ts";
import { TeamCoordinator } from "./v2.ts";

function latch() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

async function within(promise: Promise<unknown>): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("offline Pi test did not reach its next boundary")), 2_000);
    })]);
  } finally {
    clearTimeout(timer);
  }
}

/** Real SDK sessions and agent loop; the only provider is an in-process script. */
async function offlineSession(toolTurns = 0, terminal: "stop" | "error" | "aborted" = "stop") {
  const dir = mkdtempSync(join(tmpdir(), "pi-team-delivery-"));
  const entered = [latch(), latch()];
  const released = [latch(), latch()];
  const requests: string[] = [];
  const modelRuntime = await ModelRuntime.create({
    authPath: join(dir, "auth.json"), modelsPath: null,
    modelsStorePath: join(dir, "models-store.json"), allowModelNetwork: false, refreshOnCreate: false,
  });
  modelRuntime.registerProvider("offline-team", {
    api: "openai-completions", apiKey: "offline-fixture", baseUrl: "http://127.0.0.1:1",
    models: [{
      id: "fixture", name: "fixture", reasoning: false, input: ["text"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 100_000, maxTokens: 1_024,
    }],
    streamSimple: (model, context) => {
      const step = requests.push(JSON.stringify(context.messages));
      const stopReason = step <= toolTurns ? "toolUse" : terminal;
      const message: AssistantMessage = {
        role: "assistant", api: model.api, provider: model.provider, model: model.id, timestamp: Date.now(),
        stopReason,
        ...(stopReason === "error" || stopReason === "aborted" ? { errorMessage: "offline malformed stream" } : {}),
        content: stopReason === "toolUse"
          ? [{ type: "toolCall", id: `gate-${step}`, name: "gate", arguments: { slot: step } }]
          : [{ type: "text", text: "offline result" }],
        usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
      };
      const stream = createAssistantMessageEventStream();
      if (stopReason === "error" || stopReason === "aborted") {
        stream.push({ type: "error", reason: stopReason, error: message });
      } else {
        stream.push({ type: "done", reason: stopReason, message });
      }
      stream.end();
      return stream;
    },
  });
  const settingsManager = SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false } });
  const resourceLoader = new DefaultResourceLoader({
    cwd: dir, agentDir: dir, settingsManager, noExtensions: true, noSkills: true,
    noContextFiles: true, noThemes: true, noPromptTemplates: true, systemPrompt: "Offline delivery test.",
  });
  await resourceLoader.reload();
  const gate = defineTool({
    name: "gate", label: "Gate", description: "Controlled offline tool boundary.",
    parameters: Type.Object({ slot: Type.Integer({ minimum: 1, maximum: 2 }) }),
    execute: async (_id, { slot }) => {
      entered[slot - 1].resolve();
      await released[slot - 1].promise;
      return { content: [{ type: "text", text: "tool completed" }], details: {} };
    },
  });
  const { session } = await createAgentSession({
    cwd: dir, agentDir: dir, settingsManager, resourceLoader, modelRuntime,
    model: modelRuntime.getModel("offline-team", "fixture"),
    sessionManager: SessionManager.inMemory(dir), tools: ["gate"], customTools: [gate],
  });
  return {
    session, requests, entered, released,
    async cleanup() {
      for (const item of released) item.resolve();
      await session.abort();
      session.dispose();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

function team(root: AgentSession, children: AgentSession[]) {
  const tools = new Map<string, ToolDefinition>();
  const childTools = new Map<string, ToolDefinition[]>();
  const records: SubagentRecord[] = [];
  const finishes = new Map<string, RecordFinishHandler>();
  const rootDeliveries: Promise<void>[] = [];
  const runtime = {
    async createRecord(_call: string, params: SpawnParams, _signal: AbortSignal | undefined, _ctx: ExtensionContext, options: RecordCreateOptions = {}) {
      const session = children[records.length];
      assert.ok(session);
      const record = {
        id: `child-${records.length}`, description: params.description,
        finished: false, terminalStatus: null, session,
      } as SubagentRecord;
      records.push(record);
      childTools.set(record.id, options.customTools ?? []);
      if (options.onFinished) finishes.set(record.id, options.onFinished);
      return record;
    },
    scheduleBackground() {},
    backgroundState: () => "running",
    formatDuration: () => "0s",
    finalOutput: () => "child dependency ready",
    recordPostFinishError(_record: SubagentRecord, message: string) { throw new Error(message); },
  };
  const pi = {
    registerTool(tool: ToolDefinition) { tools.set(tool.name, tool); },
    registerCommand() {},
    sendMessage(message: Parameters<AgentSession["sendCustomMessage"]>[0], options: Parameters<AgentSession["sendCustomMessage"]>[1]) {
      rootDeliveries.push(root.sendCustomMessage(message, options));
    },
  } as unknown as ExtensionAPI;
  const coordinator = new TeamCoordinator(pi, runtime as unknown as SubagentRuntime);
  coordinator.register();
  const ctx = { cwd: process.cwd() } as Parameters<ToolDefinition["execute"]>[4];
  async function call(name: string, params: Record<string, unknown>, sender = "/root") {
    const index = sender === "/root" ? -1 : Number(sender.split("/").at(-1)?.replace("worker", ""));
    const tool = index < 0 ? tools.get(name) : childTools.get(records[index].id)?.find((entry) => entry.name === name);
    assert.ok(tool);
    return await tool.execute(name, params as never, undefined, undefined, ctx);
  }
  return { call, coordinator, records, finishes, rootDeliveries };
}

for (const route of ["root-child", "child-root", "peer"] as const) {
  test(`real Pi ${route} ordinary messages steer at a tool boundary and do not wake an idle recipient`, async (t) => {
    t.mock.method(globalThis, "fetch", () => { throw new Error("network is forbidden in offline delivery tests"); });
    const root = await offlineSession(route === "child-root" ? 2 : 0);
    const child0 = await offlineSession(route === "root-child" ? 2 : 0);
    const child1 = await offlineSession(route === "peer" ? 2 : 0);
    const fixture = team(root.session, [child0.session, child1.session]);
    let running: Promise<void> | undefined;
    try {
      await fixture.call("spawn_team_agent", { task_name: "worker0", message: "work" });
      await fixture.call("spawn_team_agent", { task_name: "worker1", message: "work" });
      const sender = route === "root-child" ? "/root" : "/root/worker0";
      const target = route === "child-root" ? "/root" : route === "peer" ? "/root/worker1" : "/root/worker0";
      const recipient = route === "child-root" ? root : route === "peer" ? child1 : child0;
      const delivered = latch();
      const send = recipient.session.sendCustomMessage.bind(recipient.session);
      recipient.session.sendCustomMessage = async (message, options) => {
        await send(message, options);
        delivered.resolve();
      };
      running = recipient.session.prompt("keep working");
      await within(recipient.entered[0].promise);
      await fixture.call("team_send_message", { target, message: "correction while active" }, sender);
      // No team_list, next team action, or sender completion is needed to flush.
      await within(delivered.promise);
      assert.ok(recipient.session.agent.peekQueuedMessages().some((message) => JSON.stringify(message).includes("correction while active")));
      recipient.released[0].resolve();
      await within(recipient.entered[1].promise);
      assert.match(recipient.requests[1], /correction while active/);
      assert.equal(recipient.session.isStreaming, true, "the result reaches an ongoing tool loop");
      recipient.released[1].resolve();
      await running;
      const before = recipient.requests.length;
      await fixture.call("team_send_message", { target, message: "idle context only" }, sender);
      await fixture.coordinator.flushBufferedMessages();
      await Promise.all(fixture.rootDeliveries);
      assert.equal(recipient.requests.length, before);
      assert.equal(recipient.session.isStreaming, false);
      assert.ok(recipient.session.messages.some((message) => JSON.stringify(message).includes("idle context only")));
    } finally {
      fixture.coordinator.dispose();
      for (const item of [root, child0, child1]) await item.cleanup();
      await running;
    }
  });
}

test("real Pi receives FINAL_ANSWER before a busy root finishes its tool loop", async (t) => {
  t.mock.method(globalThis, "fetch", () => { throw new Error("network is forbidden in offline delivery tests"); });
  const root = await offlineSession(2);
  const child = await offlineSession();
  const fixture = team(root.session, [child.session]);
  let running: Promise<void> | undefined;
  try {
    await fixture.call("spawn_team_agent", { task_name: "worker0", message: "work" });
    running = root.session.prompt("continue independent work");
    await within(root.entered[0].promise);
    const record = fixture.records[0];
    record.finished = true;
    record.terminalStatus = "completed";
    await fixture.finishes.get(record.id)!(record, "completed");
    await Promise.all(fixture.rootDeliveries);
    root.released[0].resolve();
    await within(root.entered[1].promise);
    assert.match(root.requests[1], /FINAL_ANSWER/);
    assert.match(root.requests[1], /child dependency ready/);
    assert.equal(root.session.isStreaming, true);
  } finally {
    fixture.coordinator.dispose();
    await root.cleanup();
    await child.cleanup();
    await running;
  }
});

test("real Pi wakes an idle root on FINAL_ANSWER", async (t) => {
  t.mock.method(globalThis, "fetch", () => { throw new Error("network is forbidden in offline delivery tests"); });
  const root = await offlineSession();
  const child = await offlineSession();
  const fixture = team(root.session, [child.session]);
  try {
    await fixture.call("spawn_team_agent", { task_name: "worker0", message: "work" });
    const record = fixture.records[0];
    record.finished = true;
    record.terminalStatus = "completed";
    await fixture.finishes.get(record.id)!(record, "completed");
    await Promise.all(fixture.rootDeliveries);
    assert.equal(root.requests.length, 1);
    assert.match(root.requests[0], /FINAL_ANSWER/);
    assert.equal(root.session.isStreaming, false);
  } finally {
    fixture.coordinator.dispose();
    await root.cleanup();
    await child.cleanup();
  }
});

test("real Pi NEW_TASK remains follow-up work after the current tool loop", async (t) => {
  t.mock.method(globalThis, "fetch", () => { throw new Error("network is forbidden in offline delivery tests"); });
  const root = await offlineSession();
  const child = await offlineSession(2);
  const fixture = team(root.session, [child.session]);
  let running: Promise<void> | undefined;
  try {
    await fixture.call("spawn_team_agent", { task_name: "worker0", message: "work" });
    running = child.session.prompt("current work");
    await within(child.entered[0].promise);
    await fixture.call("team_followup_task", { target: "/root/worker0", message: "next assignment" });
    child.released[0].resolve();
    await within(child.entered[1].promise);
    assert.doesNotMatch(child.requests[1], /next assignment/);
    child.released[1].resolve();
    await running;
    assert.equal(child.requests.length, 4);
    assert.match(child.requests[3], /NEW_TASK/);
    assert.match(child.requests[3], /next assignment/);
  } finally {
    fixture.coordinator.dispose();
    await root.cleanup();
    await child.cleanup();
    await running;
  }
});

test("real Pi resolved model failures persist failed/cancelled runtime status", async (t) => {
  t.mock.method(globalThis, "fetch", () => { throw new Error("network is forbidden in offline delivery tests"); });
  for (const stop of ["error", "aborted"] as const) {
    for (const mode of ["prompt", "custom"] as const) {
      const offline = await offlineSession(0, stop);
      const statuses: string[] = [];
      const runtime = new SubagentRuntime(
        { events: { emit() {} } } as unknown as ExtensionAPI,
        Object.assign(() => undefined, { ready: Promise.resolve() }),
        (_record, status) => { statuses.push(status); },
      );
      const record = {
        id: "offline-failure", background: true, finished: false, cancelRequested: false,
        session: offline.session, startedAt: Date.now(),
        removeAbortListener() {}, unsubscribe() {}, doneResolve() {},
      } as unknown as SubagentRecord;
      try {
        const run = mode === "prompt" ? runtime.run(record, "task") : runtime.runCustomMessage(
          record, { customType: "offline", content: "task", display: false }, { triggerTurn: true },
        );
        await assert.rejects(run, /offline malformed stream/);
        assert.deepEqual(statuses, [stop === "error" ? "failed" : "cancelled"]);
        assert.equal(record.lastError, "offline malformed stream");
      } finally {
        await offline.cleanup();
      }
    }
  }
});
