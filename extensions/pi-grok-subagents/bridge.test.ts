import assert from "node:assert/strict";
import { appendFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer, type Socket } from "node:net";
import test from "node:test";
import type { SubagentRecord } from "./bridge.ts";
import {
  createBridgeEmitter,
  latestPersistedRecordsFromFile,
  persist,
  persistedRecord,
  persistedRunBranch,
  subagentStateFile,
} from "./bridge.ts";

function record(stateFile: string): SubagentRecord {
  return {
    id: "subagent-1",
    childSessionId: "run-1",
    agentSessionId: "child-1",
    childSessionFile: "/sessions/subagent/child-1.jsonl",
    startLeafId: "start-1",
    endLeafId: "end-1",
    stateFile,
    parentSessionId: "parent-1",
    parentToolCallId: "call-1",
    prompt: "Inspect the parser",
    description: "Inspect parser",
    type: "general-purpose",
    capabilityMode: "all",
    modelId: "provider/model",
    background: false,
    startedAt: 100,
    session: {} as SubagentRecord["session"],
    turnCount: 0,
    toolCallCount: 0,
    toolsUsed: new Set<string>(),
    errorCount: 0,
    tokensUsed: 0,
    finished: false,
    terminalStatus: null,
    cancelRequested: false,
    maxTurns: 0,
    turnLimitReached: false,
    donePromise: Promise.resolve(),
    doneResolve: () => undefined,
    removeAbortListener: () => undefined,
    unsubscribe: () => undefined,
  };
}

test("bridge fails fast without the private socket endpoint", () => {
  const previousEndpoint = process.env.PI_GROK_SUBAGENT_SOCKET;
  delete process.env.PI_GROK_SUBAGENT_SOCKET;
  try {
    assert.throws(() => createBridgeEmitter(), /PI_GROK_SUBAGENT_SOCKET is required/);
  } finally {
    if (previousEndpoint !== undefined) process.env.PI_GROK_SUBAGENT_SOCKET = previousEndpoint;
  }
});

test("bridge readiness rejects an unavailable socket before child startup", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-grok-subagent-missing-"));
  const previousEndpoint = process.env.PI_GROK_SUBAGENT_SOCKET;
  process.env.PI_GROK_SUBAGENT_SOCKET = join(dir, "missing.sock");
  try {
    const emit = createBridgeEmitter();
    await assert.rejects(emit.ready);
  } finally {
    if (previousEndpoint === undefined) delete process.env.PI_GROK_SUBAGENT_SOCKET;
    else process.env.PI_GROK_SUBAGENT_SOCKET = previousEndpoint;
    rmSync(dir, { recursive: true, force: true });
  }
});

test("socket bridge preserves event order", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-grok-subagent-socket-"));
  const socketPath = join(dir, "events.sock");
  const server = createServer();
  const connection = new Promise<Socket>((resolve) => server.once("connection", resolve));
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });
  const previousEndpoint = process.env.PI_GROK_SUBAGENT_SOCKET;
  process.env.PI_GROK_SUBAGENT_SOCKET = socketPath;
  try {
    const emit = createBridgeEmitter();
    const stream = await connection;
    await emit.ready;
    const received = new Promise<Array<Record<string, unknown>>>((resolve) => {
      let buffer = "";
      stream.on("data", (chunk) => {
        buffer += chunk.toString();
        const lines = buffer.split("\n").filter(Boolean);
        if (lines.length >= 3) resolve(lines.slice(0, 3).map((line) => JSON.parse(line)));
      });
    });
    const ref = { id: "subagent-1", childSessionId: "child-1", parentSessionId: "parent-1" };
    emit(ref, "spawned", { description: "task" });
    emit(ref, "child_update", { update: { type: "assistant_delta", text: "done" } });
    emit(ref, "finished", { status: "completed" });

    const events = await received;
    assert.deepEqual(events.map((event) => (event.data as { sequence: number }).sequence), [1, 2, 3]);
    assert.deepEqual(events.map((event) => (event.data as { kind: string }).kind), ["spawned", "child_update", "finished"]);
    stream.destroy();
  } finally {
    if (previousEndpoint === undefined) delete process.env.PI_GROK_SUBAGENT_SOCKET;
    else process.env.PI_GROK_SUBAGENT_SOCKET = previousEndpoint;
    await new Promise<void>((resolve) => server.close(() => resolve()));
    rmSync(dir, { recursive: true, force: true });
  }
});

test("subagent snapshots use a separate append-only sidecar", () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-grok-subagent-state-"));
  try {
    const parentFile = join(dir, "parent.jsonl");
    const stateFile = subagentStateFile(parentFile);
    const value = record(stateFile);

    persist(value, "running");
    value.turnCount = 2;
    value.toolCallCount = 1;
    value.tokensUsed = 42;
    persist(value, "completed");
    appendFileSync(stateFile, "{truncated\n", "utf8");

    const snapshots = latestPersistedRecordsFromFile(stateFile, "parent-1");
    assert.equal(snapshots.length, 1);
    assert.equal(snapshots[0].status, "completed");
    assert.equal(snapshots[0].turnCount, 2);
    assert.equal(snapshots[0].tokensUsed, 42);
    assert.deepEqual(latestPersistedRecordsFromFile(stateFile, "another-parent"), []);

    const source = readFileSync(stateFile, "utf8");
    assert.ok(source.includes('"childSessionId":"run-1"'));
    assert.ok(source.includes('"agentSessionId":"child-1"'));
    assert.ok(!source.includes("pi-grok-subagent/v1"));
    assert.ok(!source.includes("pi-grok-subagent-state/v1"));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("persisted run replay is bounded to that run's leaf range", () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-grok-subagent-run-"));
  try {
    const childFile = join(dir, "child.jsonl");
    writeFileSync(childFile, [
      JSON.stringify({ type: "session", version: 3, id: "child-1", timestamp: "2026-01-01T00:00:00Z", cwd: dir }),
      JSON.stringify({ type: "message", id: "before", parentId: null, timestamp: "2026-01-01T00:00:01Z", message: { role: "user", content: [{ type: "text", text: "before" }] } }),
      JSON.stringify({ type: "message", id: "run-1", parentId: "before", timestamp: "2026-01-01T00:00:02Z", message: { role: "user", content: [{ type: "text", text: "run" }] } }),
      JSON.stringify({ type: "message", id: "run-2", parentId: "run-1", timestamp: "2026-01-01T00:00:03Z", message: { role: "assistant", content: [{ type: "text", text: "done" }] } }),
      "",
    ].join("\n"));
    const snapshot = {
      ...record(join(dir, "state.jsonl")),
      childSessionFile: childFile,
      startLeafId: "before",
      endLeafId: "run-2",
    };
    const ids = persistedRunBranch(snapshot).map((entry) => (entry as { id: string }).id);
    assert.deepEqual(ids, ["run-1", "run-2"]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("sidecar validation rejects unknown capability and status", () => {
  const base = {
    version: 1,
    id: "subagent-1",
    childSessionId: "run-1",
    agentSessionId: "child-1",
    childSessionFile: "/child.jsonl",
    startLeafId: null,
    endLeafId: null,
    parentSessionId: "parent-1",
    parentToolCallId: "call-1",
    prompt: "task",
    description: "task",
    type: "general-purpose",
    capabilityMode: "all",
    modelId: "provider/model",
    background: false,
    startedAt: 1,
    status: "completed",
    turnCount: 1,
    toolCallCount: 0,
    tokensUsed: 1,
  };
  assert.ok(persistedRecord(base));
  assert.ok(persistedRecord({
    ...base,
    agentPath: "/root/worker/helper",
    parentAgentPath: "/root/worker",
    team: "review",
  }));
  assert.equal(persistedRecord({ ...base, agentPath: 42 }), undefined);
  assert.equal(persistedRecord({ ...base, capabilityMode: "root" }), undefined);
  assert.equal(persistedRecord({ ...base, status: "unknown" }), undefined);
});
