import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  createAssistantMessageEventStream,
  getCurrentSystemMessage,
  getCurrentSystemPrompt,
  getCurrentTools,
  type AssistantMessage,
  type AssistantMessageEventStream,
  type Model,
  type TranscriptContext,
} from "@earendil-works/pi-ai";
import {
  createAgentSession,
  createCodemodeExtension,
  DefaultResourceLoader,
  ModelRuntime,
  SessionManager,
  SettingsManager,
  type AgentSession,
} from "@earendil-works/pi-coding-agent";
import { latestPersistedRecordsFromFile, persistedRunBranch, subagentStateFile } from "./bridge.ts";
import {
  createForkedChildSessionManager,
  FORK_BRANCH_ANCHOR_TYPE,
  FORK_CONTEXT_MESSAGE_TYPE,
  HOST_GOAL_CUSTOM_TYPE,
  isHostSingletonBridgeExtension,
  parseProcessExtensionArgs,
  SharedBranchSessionManager,
} from "./fork.ts";
import piGrokSubagents from "./index.ts";

type ScriptedResponse = {
  text?: string;
  toolCalls?: Array<{ id: string; name: string; arguments: Record<string, unknown> }>;
};

type CapturedCall = {
  sessionId: string;
  modelId: string;
  systemPrompt: string;
  sections: Record<string, string | null>;
  declaredTools: string[];
  messages: TranscriptContext["messages"];
  serialized: string;
};

function makeAssistantMessage(model: Model<any>, script: ScriptedResponse): AssistantMessage {
  const content: AssistantMessage["content"] = [];
  if (script.text) content.push({ type: "text", text: script.text });
  for (const call of script.toolCalls ?? []) {
    content.push({ type: "toolCall", id: call.id, name: call.name, arguments: call.arguments });
  }
  return {
    role: "assistant",
    api: model.api,
    provider: model.provider,
    model: model.id,
    timestamp: Date.now(),
    stopReason: (script.toolCalls?.length ?? 0) > 0 ? "toolUse" : "stop",
    content,
    usage: {
      input: 10,
      output: 5,
      cacheRead: 0,
      cacheWrite: 0,
      totalTokens: 15,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    },
  };
}

function validatePairedToolCalls(messages: readonly { role: string; content?: unknown; toolCallId?: unknown }[]): void {
  const pending = new Set<string>();
  for (const message of messages) {
    if (message.role === "system") continue;
    if (message.role === "assistant" && Array.isArray(message.content)) {
      assert.equal(pending.size, 0, `assistant appeared while prior toolCalls unresolved: ${[...pending].join(",")}`);
      for (const block of message.content) {
        if (typeof block === "object" && block !== null && (block as { type?: unknown }).type === "toolCall") {
          pending.add(String((block as { id?: unknown }).id));
        }
      }
      continue;
    }
    if (message.role === "toolResult") {
      const id = String(message.toolCallId ?? "");
      assert.ok(pending.has(id), `orphan or unexpected toolResult for ${id}`);
      pending.delete(id);
      continue;
    }
    assert.equal(
      pending.size,
      0,
      `non-toolResult role=${message.role} appeared before resolving toolCalls: ${[...pending].join(",")}`,
    );
  }
  assert.equal(pending.size, 0, `dangling trailing toolCalls: ${[...pending].join(",")}`);
}

async function createOfflineEnv(options: {
  teamMode: boolean;
  withCodemode?: boolean;
  responder: (call: CapturedCall, callIndexForSession: number) => ScriptedResponse;
}) {
  const rootDir = mkdtempSync(join(tmpdir(), "pi-subagent-fork-e2e-"));
  const cwd = join(rootDir, "workspace");
  const agentDir = join(rootDir, "agent-dir");
  const grokHome = join(rootDir, "grok-home");
  const grokProject = join(cwd, ".grok-pi");
  const sessionsDir = join(rootDir, "sessions");
  const skillsDir = join(cwd, ".pi", "skills", "domain-fork-skill");
  const pluginDir = join(rootDir, "plugins");
  const bridgeDir = join(tmpdir(), `pi-grok-goal-dummy-${Date.now()}`);

  mkdirSync(cwd, { recursive: true });
  mkdirSync(agentDir, { recursive: true });
  mkdirSync(grokHome, { recursive: true });
  mkdirSync(grokProject, { recursive: true });
  mkdirSync(sessionsDir, { recursive: true });
  mkdirSync(skillsDir, { recursive: true });
  mkdirSync(pluginDir, { recursive: true });
  mkdirSync(bridgeDir, { recursive: true });

  writeFileSync(
    join(cwd, "AGENTS.md"),
    "# Workspace Rules\nPROJECT_RULE_OMEGA: Always preserve API invariants.\n",
    "utf8",
  );
  writeFileSync(
    join(skillsDir, "SKILL.md"),
    "---\nname: domain-fork-skill\ndescription: Domain verification workflow skill for fork inheritance tests.\n---\n\nFollow DOMAIN_SKILL_MARKER.\n",
    "utf8",
  );

  const userPluginPath = join(pluginDir, "user-echo-plugin.mjs");
  writeFileSync(
    userPluginPath,
    [
      "export default function userEchoPlugin(pi) {",
      "  pi.on('before_agent_start', (event) => {",
      "    const sections = (event.systemPromptOptions.sections ??= {});",
      "    sections.reply_style = 'EMPHASIS_RULE_V1: concise bullets.';",
      "    event.systemPromptOptions.promptGuidelines = [",
      "      ...(event.systemPromptOptions.promptGuidelines ?? []),",
      "      'USER_PLUGIN_GUIDELINE_XYZ',",
      "    ];",
      "  });",
      "  pi.registerTool({",
      "    name: 'user_plugin_echo',",
      "    label: 'User Plugin Echo',",
      "    description: 'Echoes text from the inherited user plugin.',",
      "    parameters: {",
      "      type: 'object',",
      "      properties: { text: { type: 'string' } },",
      "      required: ['text'],",
      "      additionalProperties: false,",
      "    },",
      "    async execute(_id, params) {",
      "      return { content: [{ type: 'text', text: `plugin-echo:${params.text}` }], details: { echoed: params.text } };",
      "    },",
      "  });",
      "}",
      "",
    ].join("\n"),
    "utf8",
  );

  // Simulate an internal host-singleton bridge extension that must NEVER leak into forked children.
  const fakeBridgePath = join(bridgeDir, "index.mjs");
  const bridgeSentinelPath = join(rootDir, "bridge-loaded-count.txt");
  writeFileSync(
    fakeBridgePath,
    [
      "import { appendFileSync } from 'node:fs';",
      `const SENTINEL = ${JSON.stringify(bridgeSentinelPath)};`,
      "export default function fakeHostBridge(pi) {",
      "  appendFileSync(SENTINEL, 'loaded\\n', 'utf8');",
      "  pi.registerTool({",
      "    name: 'goal',",
      "    label: 'Host Goal',",
      "    description: 'Host-only goal singleton tool.',",
      "    parameters: { type: 'object', properties: {}, additionalProperties: false },",
      "    async execute() { return { content: [{ type: 'text', text: 'host-goal' }], details: {} }; },",
      "  });",
      "}",
      "",
    ].join("\n"),
    "utf8",
  );

  const socketPath = join(rootDir, "bridge.sock");
  const bridgeEvents: Array<Record<string, unknown>> = [];
  const sockets: Socket[] = [];
  const server = createServer((socket) => {
    sockets.push(socket);
    let buffer = "";
    socket.on("data", (chunk) => {
      buffer += chunk.toString("utf8");
      for (;;) {
        const newline = buffer.indexOf("\n");
        if (newline < 0) break;
        const line = buffer.slice(0, newline).trim();
        buffer = buffer.slice(newline + 1);
        if (!line) continue;
        try {
          const parsed = JSON.parse(line) as { data?: Record<string, unknown> };
          if (parsed.data) bridgeEvents.push(parsed.data);
        } catch {
          // Ignore malformed fixture line.
        }
      }
    });
  });
  await new Promise<void>((resolve) => server.listen(socketPath, resolve));

  const prevEnv = {
    PI_CODING_AGENT_DIR: process.env.PI_CODING_AGENT_DIR,
    GROK_HOME: process.env.GROK_HOME,
    GROK_PROJECT_DIR: process.env.GROK_PROJECT_DIR,
    PI_GROK_SUBAGENTS: process.env.PI_GROK_SUBAGENTS,
    PI_GROK_SUBAGENTS_V2: process.env.PI_GROK_SUBAGENTS_V2,
    PI_GROK_SUBAGENT_SOCKET: process.env.PI_GROK_SUBAGENT_SOCKET,
  };
  process.env.PI_CODING_AGENT_DIR = agentDir;
  process.env.GROK_HOME = grokHome;
  process.env.GROK_PROJECT_DIR = grokProject;
  process.env.PI_GROK_SUBAGENTS = "1";
  process.env.PI_GROK_SUBAGENTS_V2 = options.teamMode ? "1" : "0";
  process.env.PI_GROK_SUBAGENT_SOCKET = socketPath;

  const capturedBySession = new Map<string, CapturedCall[]>();
  const modelRuntime = await ModelRuntime.create({
    authPath: join(agentDir, "auth.json"),
    modelsPath: null,
    modelsStorePath: join(agentDir, "models-store.json"),
    allowModelNetwork: false,
    refreshOnCreate: false,
  });

  modelRuntime.registerProvider("offline-fork", {
    api: "openai-completions",
    apiKey: "offline-fixture",
    baseUrl: "http://127.0.0.1:1",
    models: [
      {
        id: "fork-model",
        name: "fork-model",
        reasoning: true,
        input: ["text"],
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        contextWindow: 100_000,
        maxTokens: 2_048,
      },
      {
        id: "override-model",
        name: "override-model",
        reasoning: true,
        input: ["text"],
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        contextWindow: 100_000,
        maxTokens: 2_048,
      },
    ],
    streamSimple: (model, context, streamOpts): AssistantMessageEventStream => {
      const sessionId = streamOpts?.sessionId ?? "unknown";
      const systemMsg = getCurrentSystemMessage(context.messages);
      const call: CapturedCall = {
        sessionId,
        modelId: model.id,
        systemPrompt: getCurrentSystemPrompt(context.messages),
        sections: { ...(systemMsg?.sections ?? {}) },
        declaredTools: getCurrentTools(context.messages).map((tool) => tool.name),
        messages: context.messages,
        serialized: JSON.stringify(context.messages),
      };
      validatePairedToolCalls(context.messages as unknown as Array<{ role: string; content?: unknown; toolCallId?: unknown }>);
      const existing = capturedBySession.get(sessionId) ?? [];
      existing.push(call);
      capturedBySession.set(sessionId, existing);

      const step = existing.length;
      if (step > 20) {
        throw new Error(`Offline scripted provider exceeded 20 calls for session ${sessionId}`);
      }
      const reply = makeAssistantMessage(model, options.responder(call, step));
      const stream = createAssistantMessageEventStream();
      stream.push({ type: "done", reason: reply.stopReason as "stop" | "toolUse", message: reply });
      stream.end();
      return stream;
    },
  });

  const settingsManager = SettingsManager.create(cwd, agentDir);
  settingsManager.setProjectTrusted(true);

  const resourceLoader = new DefaultResourceLoader({
    cwd,
    agentDir,
    settingsManager,
    noExtensions: false,
    noSkills: false,
    noContextFiles: false,
    noPromptTemplates: true,
    noThemes: true,
    additionalExtensionPaths: [
      userPluginPath,
      fakeBridgePath,
      ...(options.withCodemode ? ["builtin:codemode"] : []),
    ],
    extensionFactories: [
      ...(options.withCodemode
        ? [{ name: "codemode", factory: createCodemodeExtension(), replaceable: true, builtin: true }]
        : []),
      piGrokSubagents,
    ],
  });
  await resourceLoader.reload();

  const connected = once(server, "connection");
  const { session } = await createAgentSession({
    cwd,
    agentDir,
    settingsManager,
    resourceLoader,
    modelRuntime,
    model: modelRuntime.getModel("offline-fork", "fork-model"),
    thinkingLevel: "medium",
    sessionManager: SessionManager.create(cwd, sessionsDir),
  });
  await session.bindExtensions({});
  if (options.withCodemode) {
    session.setActiveToolsByName([...session.getActiveToolNames(), "codemode"]);
  }
  await connected;

  return {
    rootDir,
    cwd,
    root: session,
    capturedBySession,
    bridgeEvents,
    userPluginPath,
    fakeBridgePath,
    bridgeSentinelPath,
    async cleanup() {
      await session.abort().catch(() => undefined);
      session.dispose();
      for (const socket of sockets) socket.destroy();
      await new Promise<void>((resolve) => server.close(() => resolve()));
      for (const [key, value] of Object.entries(prevEnv)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
      rmSync(bridgeDir, { recursive: true, force: true });
      rmSync(rootDir, { recursive: true, force: true });
    },
  };
}

test("host-singleton bridge filter rejects grok-pi internal bridges while preserving user/npm/builtin extensions", () => {
  assert.equal(isHostSingletonBridgeExtension("/tmp/pi-grok-subagents-abc123/index.ts"), true);
  assert.equal(isHostSingletonBridgeExtension("/tmp/pi-grok-plan-mode-xyz/index.ts"), true);
  assert.equal(isHostSingletonBridgeExtension("/tmp/pi-grok-feature-pi_workflows-123.ts"), true);
  assert.equal(isHostSingletonBridgeExtension("/repo/extensions/pi-grok-goal/index.ts"), true);
  assert.equal(isHostSingletonBridgeExtension("C:\\Users\\u\\extensions\\pi-grok-herdr\\index.ts"), true);

  assert.equal(isHostSingletonBridgeExtension("builtin:codemode"), false);
  assert.equal(isHostSingletonBridgeExtension("/home/user/.pi/agent/npm/node_modules/@ff-labs/pi-fff"), false);
  assert.equal(isHostSingletonBridgeExtension("/repo/.pi/extensions/custom-plugin.ts"), false);

  const parsed = parseProcessExtensionArgs([
    "node",
    "pi",
    "--no-extensions",
    "--no-mcp",
    "-e",
    "/tmp/pi-grok-bash-123/index.ts",
    "--extension",
    "builtin:codemode",
    "--extension=/repo/.pi/extensions/custom.ts",
    "--skill",
    "/tmp/custom-skill",
  ]);
  assert.equal(parsed.noExtensions, true);
  assert.equal(parsed.noMcp, true);
  assert.deepEqual(parsed.cliExtensionPaths, ["builtin:codemode", "/repo/.pi/extensions/custom.ts"]);
  assert.deepEqual(parsed.cliSkillPaths, ["/tmp/custom-skill"]);
});

test("SharedBranchSessionManager shares the parent JSONL across parallel and nested forks without leaf interference", () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-fork-shared-tree-"));
  try {
    const parent = SessionManager.create(dir, dir);
    const parentFile = parent.getSessionFile()!;
    parent.appendMessage({
      role: "user",
      content: [{ type: "text", text: "root turn 1" }],
      timestamp: 1,
    });
    const aFork = parent.appendMessage({
      role: "assistant",
      api: "openai-completions",
      provider: "offline",
      model: "m",
      stopReason: "toolUse",
      content: [
        { type: "toolCall", id: "call-a", name: "spawn_subagent", arguments: { fork: true } },
        { type: "toolCall", id: "call-b", name: "spawn_subagent", arguments: { fork: true } },
      ],
      usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
      timestamp: 2,
    });

    const forkA = createForkedChildSessionManager({
      parentSessionManager: parent,
      parentSessionFile: parentFile,
      subagentDir: join(dir, "subagent"),
      cwd: dir,
      spawningToolCallId: "call-a",
      description: "Worker Alpha",
    });
    const forkB = createForkedChildSessionManager({
      parentSessionManager: parent,
      parentSessionFile: parentFile,
      subagentDir: join(dir, "subagent"),
      cwd: dir,
      spawningToolCallId: "call-b",
      description: "Worker Beta",
    });

    assert.ok(forkA instanceof SharedBranchSessionManager);
    assert.ok(forkB instanceof SharedBranchSessionManager);
    assert.equal(forkA.getSessionFile(), parentFile);
    assert.equal(forkB.getSessionFile(), parentFile);
    assert.notEqual(forkA.getSessionId(), parent.getSessionId());
    assert.notEqual(forkA.getSessionId(), forkB.getSessionId());
    // Parent leaf stays fixed on aFork while both child branches advance.
    assert.equal(parent.getLeafId(), aFork);

    // Interleave writes across forkA, parent main branch, and forkB.
    forkA.appendMessage({ role: "user", content: [{ type: "text", text: "task for alpha" }], timestamp: 3 });
    parent.appendMessage({
      role: "toolResult",
      toolCallId: "call-a",
      toolName: "spawn_subagent",
      content: [{ type: "text", text: "real alpha result" }],
      isError: false,
      timestamp: 4,
    });
    parent.appendMessage({
      role: "toolResult",
      toolCallId: "call-b",
      toolName: "spawn_subagent",
      content: [{ type: "text", text: "real beta result" }],
      isError: false,
      timestamp: 5,
    });
    forkB.appendMessage({ role: "user", content: [{ type: "text", text: "task for beta" }], timestamp: 6 });

    const parentJson = JSON.stringify(parent.buildSessionProjection().messages);
    const alphaJson = JSON.stringify(forkA.buildSessionProjection().messages);
    const betaJson = JSON.stringify(forkB.buildSessionProjection().messages);

    assert.match(parentJson, /real alpha result/);
    assert.doesNotMatch(parentJson, /task for alpha|task for beta/);
    assert.match(alphaJson, /task for alpha/);
    assert.doesNotMatch(alphaJson, /task for beta|real beta result/);
    assert.match(betaJson, /task for beta/);
    assert.doesNotMatch(betaJson, /task for alpha|real alpha result/);

    validatePairedToolCalls(forkA.buildSessionProjection().messages as unknown as Array<{ role: string; content?: unknown; toolCallId?: unknown }>);
    validatePairedToolCalls(forkB.buildSessionProjection().messages as unknown as Array<{ role: string; content?: unknown; toolCallId?: unknown }>);

    // Nested fork from forkA shares the exact same file & attaches under forkA.
    const nested = createForkedChildSessionManager({
      parentSessionManager: forkA,
      parentSessionFile: parentFile,
      subagentDir: join(dir, "subagent"),
      cwd: dir,
      description: "Nested Leaf",
      agentPath: "/root/alpha/leaf",
    });
    assert.equal(nested.getSessionFile(), parentFile);
    nested.appendMessage({ role: "user", content: [{ type: "text", text: "nested leaf task" }], timestamp: 7 });

    // Reopen from disk and inspect the persisted tree (what /tree renders).
    const reopened = SessionManager.open(parentFile);
    const allLabels = [...reopened.getEntries()]
      .filter((e) => e.type === "label")
      .map((e) => e.label);
    assert.ok(allLabels.includes("fork: Worker Alpha"));
    assert.ok(allLabels.includes("fork: Worker Beta"));
    assert.ok(allLabels.includes("fork /root/alpha/leaf: Nested Leaf"));

    const forkAnchorCount = reopened.getEntries().filter(
      (e) => e.type === "custom" && e.customType === FORK_BRANCH_ANCHOR_TYPE,
    ).length;
    assert.equal(forkAnchorCount, 3);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("createForkedChildSessionManager standalone fallback preserves compaction pointers and closes toolCalls for non-mutable mocks", () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-fork-sm-"));
  const subagentDir = join(dir, "subagent");
  try {
    const parent = SessionManager.create(dir, dir);
    const u1 = parent.appendMessage({
      role: "user",
      content: [{ type: "text", text: "first prompt" }],
      timestamp: 1,
    });
    const labelId = parent.appendLabelChange(u1, "checkpoint");
    parent.appendCustomEntry(HOST_GOAL_CUSTOM_TYPE, { active: true });
    const a1 = parent.appendMessage({
      role: "assistant",
      api: "openai-completions",
      provider: "offline",
      model: "m",
      stopReason: "stop",
      content: [{ type: "text", text: "first reply" }],
      usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
      timestamp: 2,
    });
    parent.appendCustomMessageEntry(FORK_CONTEXT_MESSAGE_TYPE, "<forked_subagent_context>stale</forked_subagent_context>", false);
    parent.appendCompaction("earlier summary", labelId, 500);
    // Simulate in-flight parallel tool calls right at execute() time: assistant message has 2 unresolved toolCalls.
    parent.appendMessage({
      role: "assistant",
      api: "openai-completions",
      provider: "offline",
      model: "m",
      stopReason: "toolUse",
      content: [
        { type: "toolCall", id: "call-read-1", name: "read", arguments: { path: "a.ts" } },
        { type: "toolCall", id: "call-spawn-2", name: "spawn_subagent", arguments: { fork: true } },
      ],
      usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
      timestamp: 3,
    });

    // Partial readonly mock lacking mutable .branch/.appendMessage triggers standalone copy fallback.
    const readonlyMock = {
      getBranch: () => parent.getBranch(),
      getLeafId: () => parent.getLeafId(),
    } as unknown as Parameters<typeof createForkedChildSessionManager>[0]["parentSessionManager"];

    const childSm = createForkedChildSessionManager({
      parentSessionManager: readonlyMock,
      parentSessionFile: parent.getSessionFile()!,
      subagentDir,
      cwd: dir,
      spawningToolCallId: "call-spawn-2",
    });

    const childBranch = childSm.getBranch();
    assert.equal(childBranch.some((e) => e.type === "label"), false);
    assert.equal(childBranch.some((e) => e.type === "custom" && e.customType === HOST_GOAL_CUSTOM_TYPE), false);
    assert.equal(
      childBranch.some((e) => e.type === "custom_message" && e.customType === FORK_CONTEXT_MESSAGE_TYPE),
      false,
    );

    const compaction = childBranch.find((e) => e.type === "compaction");
    assert.ok(compaction && compaction.type === "compaction");
    assert.equal(compaction.firstKeptEntryId, a1, "compaction pointer across skipped label/goal remaps to next kept entry");

    const projection = childSm.buildSessionProjection();
    validatePairedToolCalls(projection.messages as unknown as Array<{ role: string; content?: unknown; toolCallId?: unknown }>);
    const toolResults = projection.messages.filter((m) => m.role === "toolResult");
    assert.equal(toolResults.length, 2);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("real Pi V1 fork: root -> forked child -> forked grandchild & isolated child -> forked grandchild", async (t) => {
  t.mock.method(globalThis, "fetch", () => {
    throw new Error("network is forbidden in offline fork tests");
  });

  let childPromptCount = 0;
  let root!: AgentSession;

  const env = await createOfflineEnv({
    teamMode: false,
    responder: (call, step) => {
      if (call.sessionId === root.sessionId) {
        if (step === 1) return { text: "Acknowledged ALPHA-7749." };
        if (step === 2) {
          return {
            toolCalls: [
              { id: "root-echo-1", name: "user_plugin_echo", arguments: { text: "parallel-sibling" } },
              {
                id: "root-fork-1",
                name: "spawn_subagent",
                arguments: {
                  description: "Forked V1 child",
                  prompt: "TASK_FORKED_CHILD: run user_plugin_echo then spawn a forked grandchild.",
                  fork: true,
                },
              },
            ],
          };
        }
        if (step === 3) return { text: "First V1 fork delegation complete." };
        if (step === 4) {
          return {
            toolCalls: [
              {
                id: "root-isolated-1",
                name: "spawn_subagent",
                arguments: {
                  description: "Isolated V1 child",
                  prompt: "TASK_ISOLATED_CHILD: remember ISOLATED-CHILD-99 and spawn a forked grandchild.",
                  fork: false,
                },
              },
            ],
          };
        }
        return { text: `Root done step=${step}` };
      }

      if (call.serialized.includes("TASK_FORKED_CHILD") && !call.serialized.includes("TASK_FORKED_GRANDCHILD")) {
        if (step === 1) {
          return {
            toolCalls: [
              { id: "child-echo-1", name: "user_plugin_echo", arguments: { text: "child-ok" } },
            ],
          };
        }
        if (step === 2) {
          return {
            toolCalls: [
              {
                id: "child-spawn-gc",
                name: "spawn_subagent",
                arguments: {
                  description: "Forked V1 grandchild",
                  prompt: "TASK_FORKED_GRANDCHILD: verify CHILD-FACT-88 and root context.",
                  fork: true,
                },
              },
            ],
          };
        }
        return { text: `Forked child finished after grandchild: ${++childPromptCount}` };
      }

      if (call.serialized.includes("TASK_FORKED_GRANDCHILD")) {
        if (step === 1) {
          return {
            toolCalls: [{ id: "gc-echo-1", name: "user_plugin_echo", arguments: { text: "gc-ok" } }],
          };
        }
        return { text: "Grandchild verified root+child context." };
      }

      if (call.serialized.includes("TASK_ISOLATED_CHILD") && !call.serialized.includes("TASK_ISOLATED_GRANDCHILD")) {
        if (step === 1) {
          return {
            toolCalls: [
              {
                id: "iso-spawn-gc",
                name: "spawn_subagent",
                arguments: {
                  description: "Grandchild from isolated child",
                  prompt: "TASK_ISOLATED_GRANDCHILD: report only ISOLATED-CHILD-99 branch.",
                  fork: true,
                },
              },
            ],
          };
        }
        return { text: "Isolated child done." };
      }

      if (call.serialized.includes("TASK_ISOLATED_GRANDCHILD")) {
        return { text: "Isolated grandchild done." };
      }

      return { text: "unexpected default response" };
    },
  });
  root = env.root;

  try {
    // Only root should have loaded the simulated host-singleton bridge extension once.
    const bridgeCountBefore = readFileSync(env.bridgeSentinelPath, "utf8").trim().split("\n").length;
    assert.equal(bridgeCountBefore, 1, "host bridge loaded once on root");

    await root.prompt("Remember secret token ALPHA-7749.");
    await root.prompt("Run parallel sibling echo + forked child now.");
    await root.prompt("Now spawn a non-forked isolated child.");

    const rootChildren = latestPersistedRecordsFromFile(
      subagentStateFile(root.sessionFile!),
      root.sessionId,
    );
    assert.equal(rootChildren.length, 2, `expected 2 root children, got ${rootChildren.length}`);

    const forkedChild = rootChildren.find((r) => r.description === "Forked V1 child");
    const isolatedChild = rootChildren.find((r) => r.description === "Isolated V1 child");
    assert.ok(forkedChild && isolatedChild);

    const forkedGrandchildren = latestPersistedRecordsFromFile(
      subagentStateFile(forkedChild.childSessionFile),
      forkedChild.agentSessionId,
    );
    const isolatedGrandchildren = latestPersistedRecordsFromFile(
      subagentStateFile(isolatedChild.childSessionFile),
      isolatedChild.agentSessionId,
    );
    assert.equal(forkedGrandchildren.length, 1);
    assert.equal(isolatedGrandchildren.length, 1);
    const forkedGrandchild = forkedGrandchildren[0];
    const isolatedGrandchild = isolatedGrandchildren[0];
    assert.equal(forkedGrandchild.description, "Forked V1 grandchild");
    assert.equal(isolatedGrandchild.description, "Grandchild from isolated child");

    assert.equal(forkedChild.forked, true);
    assert.equal(forkedGrandchild.forked, true);
    assert.equal(isolatedChild.forked, undefined);
    assert.equal(isolatedGrandchild.forked, true);

    // True shared-JSONL invariants: forked child + grandchild live in root.sessionFile's tree,
    // while isolated child lives in its own file shared only with its own forked grandchild.
    assert.equal(forkedChild.childSessionFile, root.sessionFile, "forked child must share root JSONL file");
    assert.equal(forkedGrandchild.childSessionFile, root.sessionFile, "forked grandchild must share root JSONL file");
    assert.notEqual(isolatedChild.childSessionFile, root.sessionFile, "non-forked child uses its own JSONL file");
    assert.equal(isolatedGrandchild.childSessionFile, isolatedChild.childSessionFile, "forked grandchild of isolated child shares isolated child JSONL");

    // Check host bridge NEVER loaded into forked children/grandchildren!
    const bridgeCountAfter = readFileSync(env.bridgeSentinelPath, "utf8").trim().split("\n").length;
    assert.equal(bridgeCountAfter, 1, "forked children must NOT load host-singleton bridge extensions");

    // 1. Verify Root -> V1 Forked Child prompt & messages & sections & plugins
    const forkedChildCalls = env.capturedBySession.get(forkedChild.agentSessionId);
    assert.ok(forkedChildCalls && forkedChildCalls.length >= 2);
    const fcCall1 = forkedChildCalls[0];
    assert.match(fcCall1.serialized, /ALPHA-7749/, "forked child sees parent conversation history");
    assert.match(fcCall1.serialized, /parallel-sibling/, "forked child sees sibling toolCall from fork boundary");
    assert.match(fcCall1.systemPrompt, /PROJECT_RULE_OMEGA/, "forked child inherits AGENTS.md context");
    assert.match(fcCall1.systemPrompt, /domain-fork-skill/, "forked child inherits parent skills");
    assert.match(fcCall1.sections.reply_style ?? "", /EMPHASIS_RULE_V1/, "forked child keeps custom reply_style section");
    assert.match(fcCall1.sections.rules ?? "", /USER_PLUGIN_GUIDELINE_XYZ/, "forked child inherits promptGuidelines");
    assert.ok(fcCall1.sections.preamble?.includes("coding assistant running inside pi"), "preamble not wiped by customPrompt");
    assert.ok(fcCall1.sections.tools?.includes("- read:"), "tools section preserved");
    assert.ok(fcCall1.declaredTools.includes("user_plugin_echo"), "user plugin tool active in forked child");
    assert.ok(fcCall1.declaredTools.includes("spawn_subagent"), "V1 subagent tools active in forked child");
    assert.equal(fcCall1.declaredTools.includes("goal"), false, "host-only tool goal excluded from forked child");

    // Child executed user_plugin_echo successfully.
    assert.match(forkedChildCalls[1].serialized, /plugin-echo:child-ok/);

    // 2. Verify V1 Forked Child -> V1 Forked Grandchild
    const gcCalls = env.capturedBySession.get(forkedGrandchild.agentSessionId);
    assert.ok(gcCalls && gcCalls.length >= 2);
    const gcCall1 = gcCalls[0];
    assert.match(gcCall1.serialized, /ALPHA-7749/, "grandchild sees root conversation through forked parent");
    assert.match(gcCall1.serialized, /plugin-echo:child-ok/, "grandchild sees parent child's own tool results");
    assert.match(gcCall1.systemPrompt, /PROJECT_RULE_OMEGA/);
    assert.match(gcCall1.systemPrompt, /domain-fork-skill/);
    assert.match(gcCall1.sections.reply_style ?? "", /EMPHASIS_RULE_V1/);
    const envMatches = gcCall1.serialized.match(/<forked_subagent_context>/g) ?? [];
    assert.equal(envMatches.length, 1, "grandchild must receive exactly its own single fork envelope, not parent's");

    // 3. Verify bridge routing & persistedRunBranch isolation for forked child
    const forkedSpawnEvent = env.bridgeEvents.find(
      (ev) => ev.kind === "spawned" && ev.subagentId === forkedChild.id,
    );
    assert.equal(forkedSpawnEvent?.parentSessionId, root.sessionId);
    assert.equal((forkedSpawnEvent?.payload as Record<string, unknown>)?.forked, true);

    const gcSpawnEvent = env.bridgeEvents.find(
      (ev) => ev.kind === "spawned" && ev.subagentId === forkedGrandchild.id,
    );
    assert.equal(gcSpawnEvent?.parentSessionId, forkedChild.childSessionId, "child-initiated fork routes to child pane");

    const forkedChildRunEntries = persistedRunBranch(forkedChild);
    const childRunJson = JSON.stringify(forkedChildRunEntries);
    assert.doesNotMatch(childRunJson, /Remember secret token ALPHA-7749/, "UI history/replay only shows child's own turns");

    // Verify running-state fallback (endLeafId=null) on shared JSONL still scopes cleanly to forkedChild.
    const liveBranchFallback = persistedRunBranch({ ...forkedChild, endLeafId: null });
    const liveBranchJson = JSON.stringify(liveBranchFallback);
    assert.match(liveBranchJson, /TASK_FORKED_CHILD/);
    assert.doesNotMatch(
      liveBranchJson,
      /plugin-echo:gc-ok|gc-echo-1|fork: Forked V1 grandchild/,
      "running parent replay must not walk into nested grandchild branch",
    );

    // 4. Verify real Pi /tree navigation onto a forked subagent branch in the same JSONL!
    const rootOriginalLeaf = root.sessionManager.getLeafId()!;
    const rootTreeLabels = root.sessionManager.getEntries()
      .filter((e) => e.type === "label")
      .map((e) => e.label);
    assert.ok(rootTreeLabels.includes("fork: Forked V1 child"));
    assert.ok(rootTreeLabels.includes("fork: Forked V1 grandchild"));
    assert.ok(forkedChild.endLeafId);
    const navToChild = await root.navigateTree(forkedChild.endLeafId);
    assert.equal(navToChild.cancelled, false);
    assert.equal(root.sessionManager.getLeafId(), forkedChild.endLeafId);
    const navigatedJson = JSON.stringify(root.messages);
    assert.match(navigatedJson, /plugin-echo:child-ok/, "/tree jump lands cleanly inside forked subagent branch");
    assert.doesNotMatch(navigatedJson, /Now spawn a non-forked isolated child/);
    await root.navigateTree(rootOriginalLeaf);
    assert.equal(root.sessionManager.getLeafId(), rootOriginalLeaf);

    // 5. Verify Non-Forked Child -> Forked Grandchild sees child's isolated context, NOT root's!
    const isoGcCalls = env.capturedBySession.get(isolatedGrandchild.agentSessionId);
    assert.ok(isoGcCalls && isoGcCalls.length >= 1);
    assert.match(isoGcCalls[0].serialized, /ISOLATED-CHILD-99/);
    assert.doesNotMatch(isoGcCalls[0].serialized, /ALPHA-7749/, "fork from isolated child must not leak root history");
  } finally {
    await env.cleanup();
  }
});

test("real Pi V2 fork: root & child spawn_team_agent(fork=true), reactivation, and read-only spawn_team(fork=true)", async (t) => {
  t.mock.method(globalThis, "fetch", () => {
    throw new Error("network is forbidden in offline fork tests");
  });

  let root!: AgentSession;
  let rootWakeResolve!: () => void;
  const rootWokeTwice = new Promise<void>((resolve) => {
    rootWakeResolve = resolve;
  });
  let rootCalls = 0;

  const env = await createOfflineEnv({
    teamMode: true,
    responder: (call, step) => {
      if (call.sessionId === root.sessionId) {
        rootCalls += 1;
        if (rootCalls === 1) return { text: "Stored secret ROOT-V2-9001." };
        if (rootCalls === 2) {
          return {
            toolCalls: [
              {
                id: "v2-spawn-coord",
                name: "spawn_team_agent",
                arguments: {
                  task_name: "coord",
                  message: "V2_COORD_TASK: inspect user_plugin_echo and spawn a forked leaf.",
                  fork: true,
                },
              },
            ],
          };
        }
        if (rootCalls === 3) return { text: "Coord spawned asynchronously; ending turn." };
        if (rootCalls === 4) {
          // Root woken by coord's FINAL_ANSWER. Send follow-up task.
          return {
            toolCalls: [
              {
                id: "v2-followup-coord",
                name: "team_followup_task",
                arguments: {
                  target: "/root/coord",
                  message: "FOLLOWUP_AFTER_FINAL: confirm context still intact.",
                },
              },
            ],
          };
        }
        if (rootCalls === 5) return { text: "Followup queued; waiting for coord final answer." };
        if (rootCalls === 6) {
          // Root woken by coord followup FINAL_ANSWER. Test read-only spawn_team(fork=true).
          return {
            toolCalls: [
              {
                id: "v2-spawn-team-ro",
                name: "spawn_team",
                arguments: {
                  team: "research",
                  members: ["critic"],
                  task: "RO_TEAM_FORK: review ROOT-V2-9001",
                  fork: true,
                  capability_mode: "read-only",
                  timeout_ms: 10_000,
                },
              },
            ],
          };
        }
        rootWakeResolve();
        return { text: "Root V2 orchestration complete." };
      }

      if (call.serialized.includes("V2_LEAF_TASK")) {
        if (!call.serialized.includes("Recorded ready")) {
          return {
            toolCalls: [
              {
                id: "v2-leaf-ready",
                name: "team_report",
                arguments: { state: "ready", summary: "Leaf verified coord-fact and ROOT-V2-9001." },
              },
            ],
          };
        }
        return { text: "Leaf finished." };
      }

      if (call.serialized.includes("FOLLOWUP_AFTER_FINAL")) {
        if (!call.serialized.includes("Followup confirmed.")) {
          return {
            toolCalls: [
              {
                id: "v2-coord-followup-ready",
                name: "team_report",
                arguments: { state: "ready", summary: "Followup confirmed." },
              },
            ],
          };
        }
        return { text: "Coord followup turn done." };
      }

      if (call.serialized.includes("Leaf verified coord-fact")) {
        if (!call.serialized.includes("Coord + leaf verified ROOT-V2-9001.")) {
          return {
            toolCalls: [
              {
                id: "v2-coord-ready-report",
                name: "team_report",
                arguments: { state: "ready", summary: "Coord + leaf verified ROOT-V2-9001." },
              },
            ],
          };
        }
        return { text: "Coord final turn done." };
      }

      if (call.serialized.includes("V2_COORD_TASK")) {
        if (step === 1) {
          return {
            toolCalls: [
              { id: "v2-coord-echo", name: "user_plugin_echo", arguments: { text: "coord-fact" } },
              {
                id: "v2-spawn-leaf",
                name: "spawn_team_agent",
                arguments: {
                  task_name: "leaf",
                  message: "V2_LEAF_TASK: report ready immediately.",
                  fork: true,
                  capability_mode: "read-only",
                },
              },
            ],
          };
        }
        if (step === 2) {
          return {
            toolCalls: [
              {
                id: "v2-coord-wait-report",
                name: "team_report",
                arguments: { state: "waiting", summary: "Waiting for /root/coord/leaf." },
              },
            ],
          };
        }
        return { text: "Coord yielding turn for leaf result." };
      }

      if (call.serialized.includes("RO_TEAM_FORK")) {
        return { text: "Read-only forked critic done." };
      }

      return { text: `fallback step=${step}` };
    },
  });
  root = env.root;

  try {
    await root.prompt("Remember ROOT-V2-9001.");
    await root.prompt("Start forked V2 coordinator.");
    await Promise.race([
      rootWokeTwice,
      new Promise((_, reject) => setTimeout(() => reject(new Error("timed out waiting for V2 fork flow")), 5_000)),
    ]);

    const rootChildren = latestPersistedRecordsFromFile(subagentStateFile(root.sessionFile!), root.sessionId);
    const coord = rootChildren.find((r) => r.agentPath === "/root/coord");
    const critic = rootChildren.find((r) => r.agentPath === "/root/critic");
    assert.ok(coord && critic);
    const coordChildren = latestPersistedRecordsFromFile(
      subagentStateFile(coord.childSessionFile),
      coord.agentSessionId,
    );
    const leaf = coordChildren.find((r) => r.agentPath === "/root/coord/leaf");
    assert.ok(leaf);

    assert.equal(coord.forked, true);
    assert.equal(leaf.forked, true);
    assert.equal(critic.forked, true);
    assert.equal(critic.capabilityMode, "read-only");
    assert.equal(coord.childSessionFile, root.sessionFile, "V2 forked coord shares root JSONL");
    assert.equal(leaf.childSessionFile, root.sessionFile, "V2 nested forked leaf shares root JSONL");
    assert.equal(critic.childSessionFile, root.sessionFile, "V2 spawn_team(fork=true) shares root JSONL");
    const v2Labels = root.sessionManager.getEntries()
      .filter((e) => e.type === "label")
      .map((e) => e.label);
    assert.ok(v2Labels.some((l) => l?.startsWith("fork /root/coord")));
    assert.ok(v2Labels.some((l) => l?.startsWith("fork /root/coord/leaf")));
    assert.ok(v2Labels.some((l) => l?.startsWith("fork /root/critic")));

    // 1. Coord inherits root history, skills, AGENTS.md, reply_style, plugin, AND excludes root-only spawn_team.
    const coordCalls = env.capturedBySession.get(coord.agentSessionId);
    assert.ok(coordCalls && coordCalls.length >= 2);
    assert.match(coordCalls[0].serialized, /ROOT-V2-9001/);
    assert.match(coordCalls[0].serialized, /You are \/root\/coord/);
    assert.match(coordCalls[0].systemPrompt, /PROJECT_RULE_OMEGA/);
    assert.match(coordCalls[0].systemPrompt, /domain-fork-skill/);
    assert.match(coordCalls[0].sections.reply_style ?? "", /EMPHASIS_RULE_V1/);
    assert.ok(coordCalls[0].declaredTools.includes("user_plugin_echo"));
    assert.equal(coordCalls[0].declaredTools.includes("spawn_team"), false);

    // Reactivated followup keeps session history AND does NOT duplicate <forked_subagent_context>.
    const followupCall = coordCalls.at(-1)!;
    assert.match(followupCall.serialized, /FOLLOWUP_AFTER_FINAL/);
    assert.match(followupCall.serialized, /ROOT-V2-9001/);
    assert.equal((followupCall.serialized.match(/<forked_subagent_context>/g) ?? []).length, 1);

    // 2. Child-initiated V2 fork (/root/coord/leaf)
    const leafCalls = env.capturedBySession.get(leaf.agentSessionId);
    assert.ok(leafCalls && leafCalls.length >= 1);
    assert.match(leafCalls[0].serialized, /ROOT-V2-9001/);
    assert.match(leafCalls[0].serialized, /You are \/root\/coord\/leaf/);
    assert.equal(leafCalls[0].declaredTools.includes("edit"), false, "read-only fork must strip edit");
    assert.equal(leafCalls[0].declaredTools.includes("write"), false, "read-only fork must strip write");
    assert.match(leafCalls[0].serialized, /READ-ONLY CONTRACT/);

    // 3. spawn_team(..., fork=true, capability_mode="read-only")
    const criticCalls = env.capturedBySession.get(critic.agentSessionId);
    assert.ok(criticCalls && criticCalls.length >= 1);
    assert.match(criticCalls[0].serialized, /ROOT-V2-9001/);
    assert.match(criticCalls[0].systemPrompt, /PROJECT_RULE_OMEGA/);
    assert.match(criticCalls[0].sections.reply_style ?? "", /EMPHASIS_RULE_V1/);
    assert.ok(criticCalls[0].declaredTools.includes("user_plugin_echo"));
    assert.equal(criticCalls[0].declaredTools.includes("edit"), false);
    assert.equal(criticCalls[0].declaredTools.includes("write"), false);
    assert.match(criticCalls[0].serialized, /READ-ONLY CONTRACT/);
  } finally {
    await env.cleanup();
  }
});

test("real Pi V1 fork works on turn 1, supports background+overrides+builtin:codemode, and isolates persistedRunBranch", async (t) => {
  t.mock.method(globalThis, "fetch", () => {
    throw new Error("network is forbidden in offline fork tests");
  });

  let root!: AgentSession;
  let bgRecordId: string | undefined;
  let rootStep = 0;
  const env = await createOfflineEnv({
    teamMode: false,
    withCodemode: true,
    responder: (call) => {
      if (call.sessionId === root.sessionId) {
        rootStep += 1;
        if (rootStep === 1) {
          return {
            toolCalls: [
              {
                id: "t1-fork-fg",
                name: "spawn_subagent",
                arguments: {
                  description: "Turn1 fork child",
                  prompt: "Inspect TURN1_SECRET_XYZ",
                  fork: true,
                },
              },
              {
                id: "t1-fork-bg",
                name: "spawn_subagent",
                arguments: {
                  description: "Turn1 bg override",
                  prompt: "Background check TURN1_SECRET_XYZ",
                  fork: true,
                  background: true,
                  model: "offline-fork/override-model",
                  thinking_level: "low",
                  capability_mode: "read-only",
                },
              },
            ],
          };
        }
        if (rootStep === 2) {
          return { text: "Turn 1 spawns submitted." };
        }
        if (rootStep === 3) {
          return {
            toolCalls: [
              {
                id: "t1-wait-bg",
                name: "get_command_or_subagent_output",
                arguments: { task_ids: [bgRecordId!], timeout_ms: 10_000 },
              },
            ],
          };
        }
        return { text: "Turn 1 bg result collected." };
      }
      return { text: `Child finished on ${call.modelId}` };
    },
  });
  root = env.root;

  try {
    await root.prompt("Immediate task carrying TURN1_SECRET_XYZ.");
    let records = latestPersistedRecordsFromFile(subagentStateFile(root.sessionFile!), root.sessionId);
    assert.equal(records.length, 2);
    const fg = records.find((r) => r.description === "Turn1 fork child");
    const bg = records.find((r) => r.description === "Turn1 bg override");
    assert.ok(fg && bg);
    bgRecordId = bg.id;

    await root.prompt("Wait for background forked override child.");
    records = latestPersistedRecordsFromFile(subagentStateFile(root.sessionFile!), root.sessionId);
    const bgDone = records.find((r) => r.id === bg.id);
    assert.equal(bgDone?.status, "completed");
    assert.equal(bgDone?.forked, true);
    assert.equal(bgDone?.modelId, "override-model");
    assert.equal(bgDone?.thinkingLevel, "low");
    assert.equal(bgDone?.capabilityMode, "read-only");

    const fgCalls = env.capturedBySession.get(fg.agentSessionId);
    const bgCalls = env.capturedBySession.get(bg.agentSessionId);
    assert.ok(fgCalls && fgCalls.length >= 1);
    assert.ok(bgCalls && bgCalls.length >= 1);

    // Foreground child inherited turn-1 prompt, codemode builtin extension, AGENTS.md, skill, and reply_style.
    assert.match(fgCalls[0].serialized, /TURN1_SECRET_XYZ/);
    assert.equal(fgCalls[0].modelId, "fork-model");
    assert.ok(fgCalls[0].declaredTools.includes("codemode"), "forked child inherits builtin:codemode extension");
    assert.match(fgCalls[0].systemPrompt, /PROJECT_RULE_OMEGA/);
    assert.match(fgCalls[0].sections.reply_style ?? "", /EMPHASIS_RULE_V1/);

    // Background read-only child used override-model, stripped edit/write, kept user plugin & builtin:codemode, and sees TURN1_SECRET_XYZ.
    assert.equal(bgCalls[0].modelId, "override-model");
    assert.match(bgCalls[0].serialized, /TURN1_SECRET_XYZ/);
    assert.equal(bgCalls[0].declaredTools.includes("edit"), false);
    assert.equal(bgCalls[0].declaredTools.includes("write"), false);
    assert.ok(bgCalls[0].declaredTools.includes("user_plugin_echo"));
    assert.ok(bgCalls[0].declaredTools.includes("codemode"));
    assert.match(bgCalls[0].serialized, /READ-ONLY CONTRACT/);
  } finally {
    await env.cleanup();
  }
});
