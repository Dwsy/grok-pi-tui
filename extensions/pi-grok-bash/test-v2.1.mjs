import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const require = createRequire(import.meta.url);
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const esbuild = require(join(repoRoot, "pi-main/node_modules/esbuild"));
const sleep = (ms) => new Promise((resolvePromise) => setTimeout(resolvePromise, ms));

function success(text) {
  return {
    content: [{ type: "text", text }],
    details: {},
    isError: false,
  };
}

async function buildExtension() {
  const tempDir = await mkdtemp(join(tmpdir(), "pi-grok-eval-v21-"));
  const outfile = join(tempDir, "pi-grok-bash.mjs");
  const previousPackageDir = process.env.PI_PACKAGE_DIR;
  process.env.PI_PACKAGE_DIR = join(repoRoot, "pi-main/packages/coding-agent");
  await esbuild.build({
    entryPoints: [join(repoRoot, "extensions/pi-grok-bash/index.ts")],
    bundle: true,
    outfile,
    platform: "node",
    format: "esm",
    target: "node22",
    banner: {
      js: 'import { createRequire as __createRequire } from "node:module"; const require = __createRequire(import.meta.url);',
    },
    nodePaths: [
      join(homedir(), ".pi/agent/npm/node_modules"),
      join(repoRoot, "pi-main/node_modules"),
    ],
    logLevel: "silent",
  });
  const loaded = await import(`${pathToFileURL(outfile).href}?v=${Date.now()}`);
  return { register: loaded.default, EvalSessionToolBridge: loaded.EvalSessionToolBridge, tempDir, previousPackageDir };
}

async function createHarness(register, version, options = {}) {
  const registeredTools = new Map();
  const handlers = new Map();
  const previousVersion = process.env.PI_GROK_EVAL_VERSION;
  const previousBash = process.env.PI_GROK_BASH;
  const previousBuiltinTools = process.env.PI_GROK_BUILTIN_TOOLS;
  const previousExcludedTools = process.env.PI_GROK_EXCLUDE_TOOLS;
  const previousControlMeta = process.env.PI_GROK_BASH_CONTROL_META;
  const previousEvalV2Only = process.env.PI_GROK_EVAL_V2_ONLY;
  const previousBashMaxWaitMins = process.env.PI_GROK_BASH_MAX_WAIT_MINS;
  process.env.PI_GROK_EVAL_VERSION = version;
  process.env.PI_GROK_BASH = options.bashEnabled === false ? "0" : "1";
  if (options.builtinTools === undefined) delete process.env.PI_GROK_BUILTIN_TOOLS;
  else process.env.PI_GROK_BUILTIN_TOOLS = options.builtinTools;
  if (options.excludedTools === undefined) delete process.env.PI_GROK_EXCLUDE_TOOLS;
  else process.env.PI_GROK_EXCLUDE_TOOLS = options.excludedTools;
  delete process.env.PI_GROK_BASH_CONTROL_META;
  if (options.evalV2Only === true) process.env.PI_GROK_EVAL_V2_ONLY = "1";
  else delete process.env.PI_GROK_EVAL_V2_ONLY;
  if (options.bashMaxWaitMins === undefined) delete process.env.PI_GROK_BASH_MAX_WAIT_MINS;
  else process.env.PI_GROK_BASH_MAX_WAIT_MINS = String(options.bashMaxWaitMins);

  let activeToolNames = (options.activeToolNames ?? options.toolInfo?.map((tool) => tool.name) ?? []).slice();
  const pi = {
    events: { emit() {} },
    on(event, handler) {
      const existing = handlers.get(event) ?? [];
      existing.push(handler);
      handlers.set(event, existing);
    },
    registerTool(definition) {
      registeredTools.set(definition.name, definition);
    },
    getActiveTools() {
      return activeToolNames;
    },
    setActiveTools(toolNames) {
      activeToolNames = [...toolNames];
    },
    getAllTools() {
      return options.toolInfo ?? [];
    },
    invokeTool(toolName, args, signal) {
      if (!options.invokeTool) throw new Error(`Unexpected invokeTool(${toolName})`);
      return options.invokeTool(toolName, args, signal);
    },
    complete(prompt, completionOptions, signal) {
      if (!options.complete) throw new Error(`Unexpected completion(${prompt})`);
      return options.complete(prompt, completionOptions, signal);
    },
  };

  await register(pi);
  const evalTool = registeredTools.get("eval");
  assert(evalTool, "eval tool must be registered");

  return {
    evalTool,
    registeredTools,
    getActiveTools() {
      return [...activeToolNames];
    },
    async run(params, signal) {
      return evalTool.execute(
        `test-${Math.random().toString(16).slice(2)}`,
        params,
        signal,
        undefined,
        { cwd: repoRoot },
      );
    },
    async emit(event, payload) {
      for (const handler of handlers.get(event) ?? []) await handler(payload);
    },
    async close() {
      for (const handler of handlers.get("session_shutdown") ?? []) await handler();
      if (previousVersion === undefined) delete process.env.PI_GROK_EVAL_VERSION;
      else process.env.PI_GROK_EVAL_VERSION = previousVersion;
      if (previousBash === undefined) delete process.env.PI_GROK_BASH;
      else process.env.PI_GROK_BASH = previousBash;
      if (previousBuiltinTools === undefined) delete process.env.PI_GROK_BUILTIN_TOOLS;
      else process.env.PI_GROK_BUILTIN_TOOLS = previousBuiltinTools;
      if (previousExcludedTools === undefined) delete process.env.PI_GROK_EXCLUDE_TOOLS;
      else process.env.PI_GROK_EXCLUDE_TOOLS = previousExcludedTools;
      if (previousControlMeta === undefined) delete process.env.PI_GROK_BASH_CONTROL_META;
      else process.env.PI_GROK_BASH_CONTROL_META = previousControlMeta;
      if (previousEvalV2Only === undefined) delete process.env.PI_GROK_EVAL_V2_ONLY;
      else process.env.PI_GROK_EVAL_V2_ONLY = previousEvalV2Only;
      if (previousBashMaxWaitMins === undefined) delete process.env.PI_GROK_BASH_MAX_WAIT_MINS;
      else process.env.PI_GROK_BASH_MAX_WAIT_MINS = previousBashMaxWaitMins;
    },
  };
}

const { register, EvalSessionToolBridge, tempDir, previousPackageDir } = await buildExtension();
try {
  const v1 = await createHarness(register, "v1");
  try {
    assert.equal(Object.hasOwn(v1.evalTool.parameters?.properties ?? {}, "is_background"), false);
    assert.match(v1.evalTool.description, /persistent per-language Eval v1 kernel/);
    assert.match(v1.evalTool.promptSnippet, /reuse prior cells/);
    assert.match(v1.evalTool.promptGuidelines.join("\n"), /reuse prior variables and functions/);
    const js = await v1.run({ language: "js", code: "40 + 2", timeout: 2 });
    const py = await v1.run({ language: "py", code: "40 + 2", timeout: 2 });
    assert.match(js.content[0].text, /42/);
    assert.match(py.content[0].text, /42/);
    console.log("PASS 1  Eval v1 keeps JavaScript/Python foreground behavior and exposes no background job parameter");
  } finally {
    await v1.close();
  }

  const state = {
    hangStarts: 0,
    hangAborts: 0,
    parallelRunning: 0,
    parallelMax: 0,
    delayedAbortRunning: 0,
    delayedAbortMax: 0,
    trace: [],
    spawnCalls: [],
    agentRunning: 0,
    agentMax: 0,
  };
  const toolInfo = [
    {
      name: "json_tool",
      description: "Return a JSON test payload",
      parameters: {
        type: "object",
        properties: { input: { type: "string", description: "Optional probe input" } },
      },
      promptGuidelines: ["Use json_tool for bridge metadata tests."],
      sourceInfo: { path: "/test/json-tool.ts", kind: "extension" },
      executionMode: "parallel",
    },
    { name: "hang", executionMode: "parallel" },
    { name: "parallel_probe", executionMode: "parallel" },
    { name: "delayed_abort_probe", executionMode: "parallel" },
    { name: "sequential_probe" },
    { name: "spawn_subagent", executionMode: "parallel" },
  ];

  const v2 = await createHarness(register, "v2", {
    toolInfo,
    async complete(prompt, _options, signal) {
      if (signal?.aborted) throw signal.reason;
      await sleep(5);
      return { text: `completion:${prompt}`, stopReason: "stop", usage: {} };
    },
    async invokeTool(toolName, args, signal) {
      if (toolName === "json_tool") return success('{"x":1}');
      if (toolName === "hang") {
        state.hangStarts += 1;
        return new Promise((_resolve, reject) => {
          const abort = () => {
            state.hangAborts += 1;
            reject(signal?.reason instanceof Error ? signal.reason : new Error("host aborted"));
          };
          if (signal?.aborted) abort();
          else signal?.addEventListener("abort", abort, { once: true });
        });
      }
      if (toolName === "delayed_abort_probe") {
        state.delayedAbortRunning += 1;
        state.delayedAbortMax = Math.max(state.delayedAbortMax, state.delayedAbortRunning);
        try {
          await sleep(Number(args.ms ?? 80));
          return success(String(args.name));
        } finally {
          state.delayedAbortRunning -= 1;
        }
      }
      if (toolName === "parallel_probe" || toolName === "sequential_probe") {
        const name = String(args.name);
        const ms = Number(args.ms ?? 10);
        if (toolName === "parallel_probe") {
          state.parallelRunning += 1;
          state.parallelMax = Math.max(state.parallelMax, state.parallelRunning);
        }
        state.trace.push(`${name}:start`);
        try {
          await sleep(ms);
          if (signal?.aborted) throw signal.reason;
          return success(name);
        } finally {
          state.trace.push(`${name}:end`);
          if (toolName === "parallel_probe") state.parallelRunning -= 1;
        }
      }
      if (toolName === "spawn_subagent") {
        state.spawnCalls.push({ ...args });
        state.agentRunning += 1;
        state.agentMax = Math.max(state.agentMax, state.agentRunning);
        try {
          await sleep(20);
          if (signal?.aborted) throw signal.reason;
          return success(`agent:${args.prompt}`);
        } finally {
          state.agentRunning -= 1;
        }
      }
      throw new Error(`Unexpected tool ${toolName}`);
    },
  });

  try {
    const languageSchema = v2.evalTool.parameters?.properties?.language;
    assert.deepEqual(languageSchema?.enum, ["js"]);
    assert.equal(Object.hasOwn(v2.evalTool.parameters?.properties ?? {}, "is_background"), true);
    assert.match(v2.evalTool.parameters?.properties?.code?.description ?? "", /isolated Eval Bridge v2 JavaScript cell/);
    assert.match(v2.evalTool.description, /isolated lexical scope per cell/);
    assert.match(v2.evalTool.promptSnippet, /explicit store\/load persistence/);
    assert.doesNotMatch(v2.evalTool.promptSnippet, /reuse prior cells/);
    assert.match(v2.evalTool.promptGuidelines.join("\n"), /completion\(prompt, options\) is a one-shot model call/);
    assert.match(v2.evalTool.promptGuidelines.join("\n"), /fresh lexical scope/);
    console.log("PASS 2a Eval prompt bundles preserve v1 persistence and v2 isolated/store-load semantics");
    await assert.rejects(
      v2.run({ language: "py", code: "1 + 1", timeout: 1 }),
      /does not support language "py"/,
    );
    console.log("PASS 2  Eval v2 schema/runtime are JavaScript-only");

    const isolatedA = await v2.run({
      language: "js",
      code: "const reused = 40; await Promise.resolve(); reused + 2",
      timeout: 1,
    });
    const isolatedB = await v2.run({
      language: "js",
      code: "const reused = 20; await Promise.resolve(); reused + 22",
      timeout: 1,
    });
    assert.match(isolatedA.content[0].text, /42/);
    assert.match(isolatedB.content[0].text, /42/);
    await v2.run({
      language: "js",
      code: 'store("shared", { answer: 42, nested: { ok: true } })',
      timeout: 1,
    });
    const stored = await v2.run({
      language: "js",
      code: 'const reused = load("shared"); reused.answer + Number(reused.nested.ok === true) - 1',
      timeout: 1,
    });
    assert.match(stored.content[0].text, /42/);
    console.log("PASS 2b Eval v2 isolates lexical bindings per cell and persists only explicit store/load state");

    const envelope = await v2.run({
      language: "js",
      code: 'const r = await tool.json_tool({}); console.log(JSON.stringify({hasData:Object.hasOwn(r,"data"),keys:Object.keys(r).sort(),text:r.text}));',
      timeout: 2,
    });
    assert.match(envelope.content[0].text, /"hasData":false/);
    assert.match(envelope.content[0].text, /"keys":\["content","text"\]/);
    assert.match(envelope.content[0].text, /\\"x\\":1/);
    console.log("PASS 3  Host tool envelope is exactly {text, content}");

    const discovery = await v2.run({
      language: "js",
      code: `const names = Object.keys(tool);
const listed = tools.list();
const described = tools.describe("json_tool");
console.log(JSON.stringify({
  hasJson: names.includes("json_tool"),
  hasEval: names.includes("eval"),
  listedJson: listed.some(item => item.name === "json_tool"),
  searchJson: tools.search("json").map(item => item.name),
  description: tool.json_tool.description,
  schemaType: tool.json_tool.schema?.type,
  schemaMatches: JSON.stringify(tool.json_tool.schema) === JSON.stringify(described?.schema),
  sourcePath: tool.json_tool.meta?.source?.path,
}));`,
      timeout: 2,
    });
    assert.match(discovery.content[0].text, /"hasJson":true/);
    assert.match(discovery.content[0].text, /"hasEval":false/);
    assert.match(discovery.content[0].text, /"listedJson":true/);
    assert.match(discovery.content[0].text, /"searchJson":\["json_tool"\]/);
    assert.match(discovery.content[0].text, /"description":"Return a JSON test payload"/);
    assert.match(discovery.content[0].text, /"schemaType":"object"/);
    assert.match(discovery.content[0].text, /"schemaMatches":true/);
    assert.match(discovery.content[0].text, /"sourcePath":"\/test\/json-tool.ts"/);
    console.log("PASS 4  Eval v2 exposes enumerable active-tool discovery and real schema metadata");

    const skillPath = join(tempDir, "eval-search-skill.md");
    await writeFile(skillPath, "# Eval Search Skill\n\ntrusted skill body\n", "utf8");
    await v2.emit("before_agent_start", {
      systemPromptOptions: {
        skills: [
          {
            name: "eval-search-skill",
            description: "A trusted searchable Eval skill",
            filePath: skillPath,
            sourceInfo: { path: skillPath, kind: "project" },
            disableModelInvocation: false,
          },
          {
            name: "hidden-skill",
            description: "Must stay hidden from model invocation",
            filePath: join(tempDir, "hidden-skill.md"),
            sourceInfo: { path: join(tempDir, "hidden-skill.md"), kind: "project" },
            disableModelInvocation: true,
          },
        ],
      },
    });
    const skillDiscovery = await v2.run({
      language: "js",
      code: `const listedSkills = skills.list();
const describedSkill = skills.describe("eval-search-skill");
const searchedSkills = skills.search("searchable");
const skillBody = await skills.read("eval-search-skill");
let hiddenError = "";
try { await skills.read("hidden-skill"); } catch (error) { hiddenError = String(error.message || error); }
console.log(JSON.stringify({
  names: listedSkills.map(item => item.name),
  describedName: describedSkill?.name,
  searchedNames: searchedSkills.map(item => item.name),
  body: skillBody,
  hiddenDescription: skills.describe("hidden-skill"),
  hiddenError,
}));`,
      timeout: 2,
    });
    assert.match(skillDiscovery.content[0].text, /"names":\["eval-search-skill"\]/);
    assert.match(skillDiscovery.content[0].text, /"describedName":"eval-search-skill"/);
    assert.match(skillDiscovery.content[0].text, /"searchedNames":\["eval-search-skill"\]/);
    assert.match(skillDiscovery.content[0].text, /trusted skill body/);
    assert.match(skillDiscovery.content[0].text, /"hiddenDescription":null/);
    assert.match(skillDiscovery.content[0].text, /Skill \\"hidden-skill\\" is not available in this session/);
    console.log("PASS 4b Eval v2 discovers/searches/reads only Pi-loaded model-invokable skills");

    const timeoutAbortBefore = state.hangAborts;
    const timeoutStarted = Date.now();
    await assert.rejects(
      v2.run({ language: "js", code: "await tool.hang({})", timeout: 0.05 }),
      /timed out after 0\.05 seconds/,
    );
    const timeoutElapsed = Date.now() - timeoutStarted;
    await sleep(10);
    assert(state.hangAborts > timeoutAbortBefore, "hung host call must observe timeout abort");
    assert(timeoutElapsed < 500, `wall timeout took ${timeoutElapsed}ms`);
    const afterTimeout = await v2.run({ language: "js", code: "20 + 22", timeout: 1 });
    assert.match(afterTimeout.content[0].text, /42/);
    console.log(`PASS 5  Absolute wall timeout aborts host work (${timeoutElapsed}ms) and kernel restarts`);

    const outerAbortBefore = state.hangAborts;
    const outer = new AbortController();
    const outerRun = v2.run({ language: "js", code: "await tool.hang({})", timeout: 0 }, outer.signal);
    setTimeout(() => outer.abort(new Error("outer-stop")), 40);
    await assert.rejects(outerRun, /aborted; eval kernel reset/);
    await sleep(10);
    assert(state.hangAborts > outerAbortBefore, "host call must observe outer abort");
    console.log("PASS 6  Outer abort propagates through the run-scoped signal");

    state.parallelMax = 0;
    state.parallelRunning = 0;
    state.trace.length = 0;
    await v2.run({
      language: "js",
      code: `await parallel([${Array.from({ length: 6 }, (_, i) => `() => tool.parallel_probe({name:"cap${i}",ms:35})`).join(",")}])`,
      timeout: 2,
    });
    assert.equal(state.parallelMax, 4);
    console.log("PASS 7  Parallel host calls obey fixed cap=4");

    state.delayedAbortRunning = 0;
    state.delayedAbortMax = 0;
    const delayedAbort = new AbortController();
    const delayedAbortRun = v2.run({
      language: "js",
      code: `await parallel([${Array.from({ length: 4 }, (_, i) => `() => tool.delayed_abort_probe({name:\"old${i}\",ms:120})`).join(",")}])`,
      timeout: 0,
    }, delayedAbort.signal);
    await sleep(20);
    delayedAbort.abort(new Error("delayed-abort"));
    await assert.rejects(delayedAbortRun, /aborted; eval kernel reset/);
    const nextAfterAbort = v2.run({
      language: "js",
      code: 'await tool.delayed_abort_probe({name:"new",ms:5})',
      timeout: 1,
    });
    await nextAfterAbort;
    assert.equal(state.delayedAbortMax, 4, "aborted host work must retain gate slots until the underlying work settles");
    console.log("PASS 7b Abort settles the caller without releasing live host-call slots early");

    state.parallelMax = 0;
    state.parallelRunning = 0;
    state.trace.length = 0;
    await v2.run({
      language: "js",
      code: 'await parallel([() => tool.parallel_probe({name:"p1",ms:35}), () => tool.parallel_probe({name:"p2",ms:25}), () => tool.sequential_probe({name:"s3",ms:5}), () => tool.parallel_probe({name:"p4",ms:5})])',
      timeout: 2,
    });
    const at = Object.fromEntries(state.trace.map((event, index) => [event, index]));
    assert(at["s3:start"] > at["p1:end"] && at["s3:start"] > at["p2:end"]);
    assert(at["p4:start"] > at["s3:end"]);
    console.log(`PASS 8  Undefined executionMode fails closed sequential and forms FIFO barrier: ${state.trace.join(" -> ")}`);

    const orphanStartBefore = state.hangStarts;
    const orphanAbortBefore = state.hangAborts;
    const orphanResult = await v2.run({
      language: "js",
      code: 'tool.hang({}); "cell-finished"',
      timeout: 1,
    });
    assert.match(orphanResult.content[0].text, /cell-finished/);
    await sleep(20);
    const orphanStarts = state.hangStarts - orphanStartBefore;
    const orphanAborts = state.hangAborts - orphanAbortBefore;
    assert(orphanStarts === 0 || orphanAborts === orphanStarts, "cell settle must cancel queued host work or abort every started orphan");
    const afterOrphan = await v2.run({ language: "js", code: "6 * 7", timeout: 1 });
    assert.match(afterOrphan.content[0].text, /42/);
    console.log("PASS 9  Cell settlement aborts orphan host work without poisoning the next cell");

    await v2.run({ language: "js", code: 'await agent("background", {background:true, model:"demo"})', timeout: 1 });
    const backgroundArgs = state.spawnCalls.at(-1);
    assert(backgroundArgs);
    assert.equal(backgroundArgs.background, true);
    assert.equal(backgroundArgs.model, "demo");

    await v2.run({ language: "js", code: 'await agent("foreground", {background:false, model:"demo"})', timeout: 1 });
    const foregroundArgs = state.spawnCalls.at(-1);
    assert(foregroundArgs);
    assert.equal(foregroundArgs.background, false);
    assert.equal(foregroundArgs.model, "demo");

    state.agentMax = 0;
    await v2.run({
      language: "js",
      code: 'await parallel([() => agent("a"), () => agent("b")])',
      timeout: 2,
    });
    assert.equal(state.agentMax, 2);
    console.log("PASS 10 agent() forwards background handles and remains parallelizable in foreground mode");

    const completion = await v2.run({
      language: "js",
      code: 'const r = await completion("probe"); console.log(r.text);',
      timeout: 1,
    });
    assert.match(completion.content[0].text, /completion:probe/);
    console.log("PASS 11 completion() remains a one-shot parallel-safe host leaf");

    const bashTool = v2.registeredTools.get("bash");
    const bashWaitTool = v2.registeredTools.get("wait_tasks");
    const bashOutputTool = v2.registeredTools.get("get_task_output");
    assert(bashTool && bashWaitTool && bashOutputTool);
    const bashBackground = await bashTool.execute(
      "long-background-bash",
      { command: "for i in $(seq 1 3000); do printf 'x\\n'; done", task_name: "long background output", is_background: true },
      new AbortController().signal,
      undefined,
      { cwd: repoRoot },
    );
    const bashStarted = JSON.parse(bashBackground.content[0].text);
    await bashWaitTool.execute(
      "wait-long-background-bash",
      { task_ids: [bashStarted.task_id], mode: "wait_all", timeout_ms: 3000 },
      new AbortController().signal,
    );
    const bashOutputResult = await bashOutputTool.execute(
      "output-long-background-bash",
      { task_ids: [bashStarted.task_id] },
      new AbortController().signal,
    );
    const bashOutput = JSON.parse(bashOutputResult.content[0].text);
    assert.equal(bashOutput.truncated, true);
    assert.match(bashOutput.output, /Full output:/);
    const bashReturnedLines = bashOutput.output.split("\n").filter((line) => line === "x").length;
    assert(bashReturnedLines > 0 && bashReturnedLines <= 2000);
    const bashFullOutput = await readFile(bashOutput.output_file, "utf8");
    assert.equal(bashFullOutput.split("\n").filter((line) => line === "x").length, 3000);
    console.log("PASS 11b background Bash truncates model output by Pi limits while preserving full temp output");
  } finally {
    await v2.close();
  }

  const autoBackground = await createHarness(register, "v2", { bashMaxWaitMins: "0.005" });
  try {
    const bashTool = autoBackground.registeredTools.get("bash");
    const waitTool = autoBackground.registeredTools.get("wait_tasks");
    const outputTool = autoBackground.registeredTools.get("get_task_output");
    assert(bashTool && waitTool && outputTool);

    const promoted = await bashTool.execute(
      "auto-background-bash",
      { command: "sleep 2; printf 'auto-background-done\\n'", task_name: "auto background threshold" },
      new AbortController().signal,
      undefined,
      { cwd: repoRoot },
    );
    assert.equal(promoted.details.background, true);
    assert.match(promoted.details.taskId, /^bash-/);

    const outputCapped = await outputTool.execute(
      "auto-background-output-cap",
      { task_ids: [promoted.details.taskId], timeout_ms: 5000 },
      new AbortController().signal,
    );
    assert.equal(JSON.parse(outputCapped.content[0].text).status, "running");

    const waitCapped = await waitTool.execute(
      "auto-background-wait-cap",
      { task_ids: [promoted.details.taskId], mode: "wait_all", timeout_ms: 5000 },
      new AbortController().signal,
    );
    assert.equal(JSON.parse(waitCapped.content[0].text).results[0].status, "running");

    let finalStatus = "running";
    for (let attempt = 0; attempt < 6 && finalStatus === "running"; attempt += 1) {
      const waited = await waitTool.execute(
        `auto-background-rewait-${attempt}`,
        { task_ids: [promoted.details.taskId], mode: "wait_all", timeout_ms: 5000 },
        new AbortController().signal,
      );
      finalStatus = JSON.parse(waited.content[0].text).results[0].status;
    }
    assert.equal(finalStatus, "completed");
    const completed = await outputTool.execute(
      "auto-background-completed",
      { task_ids: [promoted.details.taskId] },
      new AbortController().signal,
    );
    assert.match(JSON.parse(completed.content[0].text).output, /auto-background-done/);
    console.log("PASS 11c foreground Bash auto-backgrounds once and both blocking task APIs return at the shared max-wait cap");
  } finally {
    await autoBackground.close();
  }

  const evalOnly = await createHarness(register, "v2", { bashEnabled: false });
  try {
    assert.deepEqual([...evalOnly.registeredTools.keys()], ["eval", "get_task_output", "wait_tasks", "kill_task"]);
    const background = await evalOnly.run({
      language: "js",
      code: 'const until = Date.now() + 120; while (Date.now() < until) {} console.log("background-done")',
      title: "background eval probe",
      is_background: true,
      timeout: 2,
    });
    const started = JSON.parse(background.content[0].text);
    assert.match(started.task_id, /^eval-/);
    assert.equal(started.status, "running");

    const foreground = await evalOnly.run({ language: "js", code: "6 * 7", timeout: 1 });
    assert.match(foreground.content[0].text, /42/);

    const waitTool = evalOnly.registeredTools.get("wait_tasks");
    const outputTool = evalOnly.registeredTools.get("get_task_output");
    const killTool = evalOnly.registeredTools.get("kill_task");
    assert(waitTool && outputTool && killTool);
    const waited = await waitTool.execute(
      "wait-eval-task",
      { task_ids: [started.task_id], mode: "wait_all", timeout_ms: 1000 },
      new AbortController().signal,
    );
    const waitedBody = JSON.parse(waited.content[0].text);
    assert.equal(waitedBody.results[0].status, "completed");
    assert.match(waitedBody.results[0].output, /background-done/);
    assert.equal(waitedBody.results[0].kind, "eval");

    const largeEval = await evalOnly.run({
      language: "js",
      code: 'for (let i = 0; i < 3000; i++) console.log(String(i).padStart(4, "0") + ":" + "x".repeat(24)); "eval-tail"',
      title: "large eval output",
      is_background: true,
      timeout: 2,
    });
    const largeEvalStarted = JSON.parse(largeEval.content[0].text);
    await waitTool.execute(
      "wait-large-eval-task",
      { task_ids: [largeEvalStarted.task_id], mode: "wait_all", timeout_ms: 3000 },
      new AbortController().signal,
    );
    const largeEvalOutputResult = await outputTool.execute(
      "output-large-eval-task",
      { task_ids: [largeEvalStarted.task_id] },
      new AbortController().signal,
    );
    const largeEvalOutput = JSON.parse(largeEvalOutputResult.content[0].text);
    assert.equal(largeEvalOutput.truncated, true);
    assert.match(largeEvalOutput.output, /2999:/);
    assert.match(largeEvalOutput.output, /Full output:/);
    assert(Buffer.byteLength(largeEvalOutput.output, "utf8") < 60 * 1024);
    const largeEvalFullOutput = await readFile(largeEvalOutput.output_file, "utf8");
    assert.match(largeEvalFullOutput, /^0000:/);
    assert.match(largeEvalFullOutput, /2999:/);
    assert.match(largeEvalFullOutput, /eval-tail/);
    assert(Buffer.byteLength(largeEvalFullOutput, "utf8") > 80 * 1024);
    console.log("PASS 12b background Eval externalizes full output and returns only a bounded Pi-style tail");

    const longBackground = await evalOnly.run({
      language: "js",
      code: 'const until = Date.now() + 5000; while (Date.now() < until) {} console.log("should-not-finish")',
      is_background: true,
      timeout: 0,
    });
    const longStarted = JSON.parse(longBackground.content[0].text);
    const killed = await killTool.execute("kill-eval-task", { task_id: longStarted.task_id });
    assert.match(killed.content[0].text, /"outcome": "killed"/);
    await waitTool.execute(
      "wait-killed-eval-task",
      { task_ids: [longStarted.task_id], mode: "wait_all", timeout_ms: 1000 },
      new AbortController().signal,
    );
    const killedOutput = await outputTool.execute(
      "output-killed-eval-task",
      { task_ids: [longStarted.task_id] },
      new AbortController().signal,
    );
    assert.equal(JSON.parse(killedOutput.content[0].text).status, "cancelled");
    console.log("PASS 12 pi_bash=off keeps Eval v2 background task start/wait/get/kill and foreground kernel independence");
  } finally {
    await evalOnly.close();
  }

  const builtinBashOff = await createHarness(register, "v2", {
    builtinTools: "read,edit,write,eval",
  });
  try {
    assert.deepEqual([...builtinBashOff.registeredTools.keys()], ["eval", "get_task_output", "wait_tasks", "kill_task"]);
    console.log("PASS 13 builtin Bash=off suppresses enhanced Bash but keeps Eval v2 task management");
  } finally {
    await builtinBashOff.close();
  }

  const lifecycle = [];
  let fallbackActiveTools = ["notes_list", "eval"];
  const fallbackPi = {
    getActiveTools() {
      return fallbackActiveTools;
    },
    getAllTools() {
      return [{ name: "notes_list", executionMode: "parallel" }];
    },
  };
  assert.equal(Object.hasOwn(fallbackPi, "invokeTool"), false);
  const bridge = new EvalSessionToolBridge(fallbackPi);
  const runner = {
    getActiveTools() {
      return ["notes_list", "eval"];
    },
    createContext() {
      return { cwd: repoRoot };
    },
    async emit(event) {
      lifecycle.push(event.type);
    },
    async emitToolCall(event) {
      lifecycle.push(event.type);
      return undefined;
    },
    async emitToolResult(event) {
      lifecycle.push(event.type);
      return undefined;
    },
  };
  const notesTool = {
    definition: {
      name: "notes_list",
      description: "test notes tool",
      parameters: { type: "object", properties: {} },
      executionMode: "parallel",
      async execute(_toolCallId, args, _signal, _onUpdate, ctx) {
        assert.equal(ctx.cwd, repoRoot);
        return success(JSON.stringify({ notes: [{ name: "probe" }], limit: args.limit }));
      },
    },
    sourceInfo: { path: "/test/pi-notes/index.ts" },
  };
  bridge.observeRegisteredTools([notesTool], runner);
  const fallbackResult = await bridge.invoke("notes_list", { limit: 1 }, new AbortController().signal);
  assert.match(fallbackResult.content[0].text, /\"probe\"/);
  assert.deepEqual(lifecycle, [
    "tool_execution_start",
    "tool_call",
    "tool_result",
    "tool_execution_end",
  ]);
  console.log("PASS 14 Eval v2 invokes captured extension tools without ExtensionAPI.invokeTool");

  const previousBridgeEvalV2Only = process.env.PI_GROK_EVAL_V2_ONLY;
  try {
    fallbackActiveTools = ["eval"];
    delete process.env.PI_GROK_EVAL_V2_ONLY;
    await assert.rejects(
      () => bridge.invoke("notes_list", { limit: 1 }, new AbortController().signal),
      /inactive tool "notes_list"/,
    );

    process.env.PI_GROK_EVAL_V2_ONLY = "1";
    const evalOnlyPi = {
      ...fallbackPi,
      async invokeTool() {
        throw new Error("eval-v2-only nested calls must bypass the native active-tool gate");
      },
    };
    const evalOnlyBridge = new EvalSessionToolBridge(evalOnlyPi);
    evalOnlyBridge.observeRegisteredTools([notesTool], runner);
    assert.deepEqual(evalOnlyBridge.catalog().map((tool) => tool.name), ["notes_list"]);
    const evalOnlyResult = await evalOnlyBridge.invoke(
      "notes_list",
      { limit: 1 },
      new AbortController().signal,
    );
    assert.match(evalOnlyResult.content[0].text, /\"probe\"/);
  } finally {
    fallbackActiveTools = ["notes_list", "eval"];
    if (previousBridgeEvalV2Only === undefined) delete process.env.PI_GROK_EVAL_V2_ONLY;
    else process.env.PI_GROK_EVAL_V2_ONLY = previousBridgeEvalV2Only;
  }
  console.log("PASS 15 eval-v2-only exposes registered tools only inside Eval while normal v2 still rejects inactive tools");

  const isolated = await createHarness(register, "v2", {
    evalV2Only: true,
    toolInfo: [{ name: "notes_list" }, { name: "eval" }],
    activeToolNames: ["notes_list", "eval"],
  });
  try {
    assert.deepEqual(isolated.getActiveTools(), ["notes_list", "eval"]);
    await isolated.emit("session_start");
    assert.deepEqual(isolated.getActiveTools(), ["eval"]);
  } finally {
    await isolated.close();
  }
  console.log("PASS 16 eval-v2-only collapses only the top-level active tool set at session start");
} finally {
  await rm(tempDir, { recursive: true, force: true });
  if (previousPackageDir === undefined) delete process.env.PI_PACKAGE_DIR;
  else process.env.PI_PACKAGE_DIR = previousPackageDir;
}

console.log("\nEval v2.1 focused production regression: PASS");
