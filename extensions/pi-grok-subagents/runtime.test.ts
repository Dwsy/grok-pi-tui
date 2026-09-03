import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { BridgeEmitter, SubagentRecord } from "./bridge.ts";
import { profileFor, selectedDefinition } from "./definitions.ts";
import { SubagentRuntime } from "./runtime.ts";
import { SUBAGENT_REPLAY_COMMAND } from "./shared.ts";
import { registerV1Tools } from "./tools-v1.ts";

const noopBridge: BridgeEmitter = Object.assign(() => undefined, { ready: Promise.resolve() });
const noopPersist = () => undefined;

function captureBridge(events: Array<{ kind: string; payload: Record<string, unknown>; replay: boolean }>): BridgeEmitter {
  return Object.assign(
    (_record: unknown, kind: string, payload: Record<string, unknown>, replay = false) => {
      events.push({ kind, payload, replay });
    },
    { ready: Promise.resolve() },
  ) as BridgeEmitter;
}

function fakePi(): ExtensionAPI {
  return {
    events: { emit: () => undefined },
  } as unknown as ExtensionAPI;
}

test("runtime subscription mutates the canonical SubagentRecord", () => {
  const runtime = new SubagentRuntime(fakePi(), noopBridge, noopPersist);
  let subscriber: ((event: any) => void) | undefined;
  const prompts: Array<{ text: string; options: unknown }> = [];
  const record = {
    id: "runtime-test",
    session: {
      subscribe: (handler: (event: any) => void) => {
        subscriber = handler;
        return () => undefined;
      },
      prompt: async (text: string, options: unknown) => {
        prompts.push({ text, options });
      },
    },
    turnCount: 0,
    toolCallCount: 0,
    toolsUsed: new Set<string>(),
    errorCount: 0,
    tokensUsed: 0,
    maxTurns: 1,
    turnLimitReached: false,
  } as unknown as SubagentRecord;

  const unsubscribe = (runtime as any).subscribeRecord(record) as () => void;
  assert.ok(subscriber);
  subscriber!({ type: "tool_execution_start", toolName: "read" });
  subscriber!({ type: "tool_execution_end", isError: true });
  subscriber!({ type: "message_end", message: { role: "assistant", usage: { input: 7, output: 3 } } });
  subscriber!({ type: "turn_end" });

  assert.equal(record.toolCallCount, 1);
  assert.deepEqual([...record.toolsUsed], ["read"]);
  assert.equal(record.errorCount, 1);
  assert.equal(record.tokensUsed, 10);
  assert.equal(record.turnCount, 1);
  assert.equal(record.turnLimitReached, true);
  assert.equal(prompts.length, 1);
  assert.match(prompts[0].text, /configured soft turn limit \(1\)/);
  assert.match(prompts[0].text, /reminder, not a hard stop/);
  assert.doesNotMatch(prompts[0].text, /Stop all further tool calls|Do not make any more tool calls/);
  unsubscribe();
});

test("session_start does not replay before the session/load barrier", () => {
  const events: string[] = [];
  const bridge: BridgeEmitter = Object.assign(
    (_record: any, kind: string) => { events.push(kind); },
    { ready: Promise.resolve() },
  );
  const runtime = new SubagentRuntime(fakePi(), bridge, noopPersist);
  runtime.onSessionStart({} as Parameters<SubagentRuntime["onSessionStart"]>[0]);
  assert.deepEqual(events, []);
});

test("V1 registers the hidden post-load replay command", () => {
  const commands: string[] = [];
  const pi = {
    registerCommand: (name: string) => { commands.push(name); },
    registerTool: () => undefined,
  } as unknown as ExtensionAPI;
  const runtime = new SubagentRuntime(fakePi(), noopBridge, noopPersist);
  registerV1Tools(pi, runtime);
  assert.ok(commands.includes(SUBAGENT_REPLAY_COMMAND));
});

test("reactivation keeps the Pi session but rotates the Pager child identity", () => {
  const events: Array<{ kind: string; payload: Record<string, unknown>; replay: boolean }> = [];
  const runtime = new SubagentRuntime(fakePi(), captureBridge(events), noopPersist);
  const previous = {
    id: "run-old",
    childSessionId: "run-old",
    agentSessionId: "pi-child",
    childSessionFile: "/child.jsonl",
    startLeafId: "old-start",
    endLeafId: "old-end",
    stateFile: "/state.jsonl",
    parentSessionId: "parent",
    parentToolCallId: "call-old",
    prompt: "old",
    description: "agent",
    type: "explore",
    capabilityMode: "read-only",
    modelId: "model",
    background: true,
    session: {
      sessionManager: { getLeafId: () => "reactivation-boundary" },
      subscribe: () => () => undefined,
    },
    finished: true,
    maxTurns: 0,
  } as unknown as SubagentRecord;

  const next = runtime.resumeRecord(previous, "call-new", "continue", undefined);

  assert.equal(next.childSessionId, next.id);
  assert.notEqual(next.childSessionId, previous.childSessionId);
  assert.equal(next.agentSessionId, previous.agentSessionId);
  assert.equal(next.startLeafId, "reactivation-boundary");
  assert.equal(events[0].kind, "spawned");
});

test("finished run output stays stable when a reused child session later changes", () => {
  const runtime = new SubagentRuntime(fakePi(), noopBridge, noopPersist);
  const record = {
    finished: true,
    finalOutputText: "first run result",
    session: {
      messages: [{ role: "assistant", content: [{ type: "text", text: "later run result" }] }],
    },
  } as unknown as SubagentRecord;
  assert.equal(runtime.finalOutput(record), "first run result");
});

test("a running cancel remains cancelled when prompt resolves normally", async () => {
  const events: Array<{ kind: string; payload: Record<string, unknown>; replay: boolean }> = [];
  const runtime = new SubagentRuntime(fakePi(), captureBridge(events), noopPersist);
  const record = {
    id: "cancel-run",
    childSessionId: "cancel-run",
    agentSessionId: "child-session",
    childSessionFile: "/child.jsonl",
    startLeafId: null,
    endLeafId: null,
    stateFile: "/state.jsonl",
    parentSessionId: "parent",
    parentToolCallId: "call",
    prompt: "task",
    description: "task",
    type: "explore",
    capabilityMode: "read-only",
    modelId: "model",
    background: true,
    startedAt: Date.now(),
    session: {
      prompt: async () => undefined,
      messages: [],
      sessionManager: { getLeafId: () => "leaf" },
    },
    turnCount: 0,
    toolCallCount: 0,
    toolsUsed: new Set<string>(),
    errorCount: 0,
    tokensUsed: 0,
    finished: false,
    terminalStatus: null,
    cancelRequested: true,
    maxTurns: 0,
    turnLimitReached: false,
    donePromise: Promise.resolve(),
    doneResolve: () => undefined,
    removeAbortListener: () => undefined,
    unsubscribe: () => undefined,
  } as unknown as SubagentRecord;

  await assert.rejects(runtime.run(record, "task"), /cancelled/);
  assert.equal(record.terminalStatus, "cancelled");
  assert.equal(events.filter((event) => event.kind === "finished").length, 1);
  assert.equal(events.find((event) => event.kind === "finished")?.payload.status, "cancelled");
});

test("missing replay transcript emits exactly one failed terminal", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-grok-replay-missing-"));
  try {
    const parentFile = join(dir, "parent.jsonl");
    const stateFile = `${parentFile}.subagents.jsonl`;
    writeFileSync(stateFile, `${JSON.stringify({
      version: 1,
      id: "run-1",
      childSessionId: "run-1",
      agentSessionId: "child-1",
      childSessionFile: join(dir, "missing-child.jsonl"),
      startLeafId: null,
      endLeafId: null,
      parentSessionId: "parent-1",
      parentToolCallId: "call-1",
      prompt: "task",
      description: "task",
      type: "explore",
      capabilityMode: "read-only",
      modelId: "model",
      background: true,
      startedAt: 1,
      status: "completed",
      turnCount: 1,
      toolCallCount: 0,
      tokensUsed: 1,
    })}\n`);
    const events: Array<{ kind: string; payload: Record<string, unknown>; replay: boolean }> = [];
    const runtime = new SubagentRuntime(fakePi(), captureBridge(events), noopPersist);
    await runtime.replayPersisted({
      sessionManager: {
        getSessionFile: () => parentFile,
        getSessionId: () => "parent-1",
      },
    } as unknown as Parameters<SubagentRuntime["replayPersisted"]>[0]);

    assert.deepEqual(events.map((event) => event.kind), ["spawned", "finished"]);
    assert.equal(events[1].payload.status, "failed");
    assert.match(String(events[1].payload.error), /child transcript is unavailable/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("load replay does not cancel a live in-process child", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-grok-live-replay-"));
  try {
    const parentFile = join(dir, "parent.jsonl");
    const childFile = join(dir, "child.jsonl");
    writeFileSync(childFile, [
      JSON.stringify({ type: "session", version: 3, id: "agent-live", timestamp: "2026-01-01T00:00:00Z", cwd: dir }),
      JSON.stringify({ type: "message", id: "before", parentId: null, timestamp: "2026-01-01T00:00:01Z", message: { role: "user", content: [{ type: "text", text: "before" }] } }),
      JSON.stringify({ type: "message", id: "live-message", parentId: "before", timestamp: "2026-01-01T00:00:02Z", message: { role: "assistant", content: [{ type: "text", text: "live" }] } }),
      "",
    ].join("\n"));
    writeFileSync(`${parentFile}.subagents.jsonl`, `${JSON.stringify({
      version: 1,
      id: "live-run",
      childSessionId: "live-run",
      agentSessionId: "agent-live",
      childSessionFile: childFile,
      startLeafId: "before",
      endLeafId: null,
      parentSessionId: "parent-1",
      parentToolCallId: "call-live",
      prompt: "task",
      description: "live",
      type: "explore",
      capabilityMode: "read-only",
      modelId: "model",
      background: true,
      startedAt: 1,
      status: "running",
      turnCount: 0,
      toolCallCount: 0,
      tokensUsed: 0,
    })}\n`);
    const events: Array<{ kind: string; payload: Record<string, unknown>; replay: boolean }> = [];
    const runtime = new SubagentRuntime(fakePi(), captureBridge(events), noopPersist);
    runtime.records.set("live-run", { id: "live-run", finished: false } as unknown as SubagentRecord);
    await runtime.replayPersisted({
      sessionManager: {
        getSessionFile: () => parentFile,
        getSessionId: () => "parent-1",
      },
    } as unknown as Parameters<SubagentRuntime["replayPersisted"]>[0]);

    assert.equal(events[0].kind, "spawned");
    assert.ok(events.some((event) => event.kind === "child_update"));
    assert.ok(!events.some((event) => event.kind === "finished"));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("recovery replay settles only running orphan records without transcript duplication", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-grok-recovery-"));
  try {
    const parentFile = join(dir, "parent.jsonl");
    const stateFile = `${parentFile}.subagents.jsonl`;
    const snapshot = (id: string, status: "running" | "completed") => ({
      version: 1,
      id,
      childSessionId: id,
      agentSessionId: `agent-${id}`,
      childSessionFile: join(dir, `${id}.jsonl`),
      startLeafId: null,
      endLeafId: null,
      parentSessionId: "parent-1",
      parentToolCallId: `call-${id}`,
      prompt: "task",
      description: id,
      type: "explore",
      capabilityMode: "read-only",
      modelId: "model",
      background: true,
      startedAt: 1,
      status,
      turnCount: 1,
      toolCallCount: 0,
      tokensUsed: 1,
    });
    writeFileSync(stateFile, `${JSON.stringify(snapshot("running-run", "running"))}\n${JSON.stringify(snapshot("done-run", "completed"))}\n`);
    const events: Array<{ kind: string; payload: Record<string, unknown>; replay: boolean }> = [];
    const runtime = new SubagentRuntime(fakePi(), captureBridge(events), noopPersist);
    await runtime.replayPersisted({
      sessionManager: {
        getSessionFile: () => parentFile,
        getSessionId: () => "parent-1",
      },
    } as unknown as Parameters<SubagentRuntime["replayPersisted"]>[0], "recovery", "request-1");

    assert.deepEqual(events.map((event) => event.kind), ["spawned", "finished", "replay_complete"]);
    assert.equal(events[0].replay, false);
    assert.equal(events[1].payload.status, "cancelled");
    assert.equal(events[2].payload.requestId, "request-1");
    const persisted = readFileSync(stateFile, "utf8").trim().split("\n").map(JSON.parse);
    assert.equal(persisted.at(-1).status, "cancelled");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("queued cancellation removes work before it can start", async () => {
  const runtime = new SubagentRuntime(fakePi(), noopBridge, noopPersist);
  const blockers: Array<() => void> = [];
  const started: string[] = [];
  const records = Array.from({ length: 5 }, (_, index) => ({
    id: `queued-${index}`,
    background: true,
    finished: false,
    cancelRequested: false,
    terminalStatus: null,
    removeAbortListener: () => undefined,
    unsubscribe: () => undefined,
    doneResolve: () => undefined,
    session: {
      abort: () => undefined,
      messages: [],
      sessionManager: { getLeafId: () => "leaf" },
    },
  } as unknown as SubagentRecord));

  for (let index = 0; index < 4; index += 1) {
    runtime.scheduleBackgroundTask(records[index], async () => {
      started.push(records[index].id);
      await new Promise<void>((resolve) => blockers.push(resolve));
    });
  }
  const queuedState = runtime.scheduleBackgroundTask(records[4], async () => {
    started.push(records[4].id);
  });
  assert.equal(queuedState, "queued");
  assert.equal(runtime.backgroundState(records[4]), "queued");

  runtime.cancel(records[4]);
  assert.equal(records[4].finished, true);
  assert.equal(records[4].terminalStatus, "cancelled");
  for (const resolve of blockers) resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(started.includes(records[4].id), false);
});


test("profile lookup is case-insensitive and unknown agent definitions do not resolve", () => {
  assert.equal(profileFor("Explore").capabilityMode, "execute");
  assert.equal(profileFor("EXPLORE").type, "explore");
  assert.equal(selectedDefinition(process.cwd(), "definitely-not-a-real-agent"), undefined);
});

test("pre-cancelled background scheduling finalizes instead of leaving a zombie record", () => {
  const runtime = new SubagentRuntime(fakePi(), noopBridge, noopPersist);
  let resolved = false;
  const record = {
    id: "pre-cancelled",
    background: true,
    startedAt: Date.now(),
    finished: false,
    cancelRequested: true,
    terminalStatus: null,
    turnCount: 0,
    toolCallCount: 0,
    toolsUsed: new Set<string>(),
    errorCount: 0,
    tokensUsed: 0,
    removeAbortListener: () => undefined,
    unsubscribe: () => undefined,
    doneResolve: () => { resolved = true; },
    session: {
      messages: [],
      abort: () => undefined,
      sessionManager: { getLeafId: () => "leaf" },
    },
  } as unknown as SubagentRecord;

  let started = false;
  const state = runtime.scheduleBackgroundTask(record, async () => { started = true; });
  assert.equal(state, "skipped");
  assert.equal(started, false);
  assert.equal(record.finished, true);
  assert.equal(record.terminalStatus, "cancelled");
  assert.equal(resolved, true);
});


test("malformed project agent override shadows inherited built-in definition", () => {
  const root = mkdtempSync(join(tmpdir(), "pi-grok-agent-shadow-"));
  const previousProject = process.env.GROK_PROJECT_DIR;
  try {
    process.env.GROK_PROJECT_DIR = root;
    mkdirSync(join(root, "agents"), { recursive: true });
    writeFileSync(join(root, "agents", "explore.md"), "---\nnot: [valid\n---\n");
    assert.equal(selectedDefinition(process.cwd(), "explore"), undefined);
  } finally {
    if (previousProject === undefined) delete process.env.GROK_PROJECT_DIR;
    else process.env.GROK_PROJECT_DIR = previousProject;
    rmSync(root, { recursive: true, force: true });
  }
});
