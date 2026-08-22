import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { SubagentRecord } from "./bridge.ts";
import { profileFor, selectedDefinition } from "./definitions.ts";
import { SubagentRuntime } from "./runtime.ts";

function fakePi(): ExtensionAPI {
  return {
    appendEntry: () => undefined,
    events: { emit: () => undefined },
  } as unknown as ExtensionAPI;
}

test("runtime subscription mutates the canonical SubagentRecord", () => {
  const runtime = new SubagentRuntime(fakePi());
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
  assert.match(prompts[0].text, /maximum number of turns allowed \(1\)/);
  unsubscribe();
});

test("finished run output stays stable when a reused child session later changes", () => {
  const runtime = new SubagentRuntime(fakePi());
  const record = {
    finished: true,
    finalOutputText: "first run result",
    session: {
      messages: [{ role: "assistant", content: [{ type: "text", text: "later run result" }] }],
    },
  } as unknown as SubagentRecord;
  assert.equal(runtime.finalOutput(record), "first run result");
});

test("queued cancellation removes work before it can start", async () => {
  const runtime = new SubagentRuntime(fakePi());
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
    session: { abort: () => undefined, messages: [] },
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
  const runtime = new SubagentRuntime(fakePi());
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
    session: { messages: [], abort: () => undefined },
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
