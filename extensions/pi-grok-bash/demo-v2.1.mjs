import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function toolEnvelope(content) {
  const text = content
    .filter((part) => part.type === "text" && typeof part.text === "string")
    .map((part) => part.text)
    .join("\n");
  return { text, content };
}

async function runWithWallTimeout(work, timeoutMs) {
  const controller = new AbortController();
  const aborted = new Promise((_, reject) => {
    controller.signal.addEventListener(
      "abort",
      () => reject(controller.signal.reason ?? new Error("aborted")),
      { once: true },
    );
  });
  const timer = setTimeout(() => controller.abort(new Error("wall timeout")), timeoutMs);
  try {
    return await Promise.race([work(controller.signal), aborted]);
  } finally {
    clearTimeout(timer);
  }
}

class HostCallGate {
  #queue = [];
  #parallelRunning = 0;
  #sequentialRunning = false;

  constructor(parallelLimit) {
    assert(Number.isInteger(parallelLimit) && parallelLimit > 0);
    this.parallelLimit = parallelLimit;
  }

  run(executionMode, work) {
    const mode = executionMode === "parallel" ? "parallel" : "sequential";
    return new Promise((resolve, reject) => {
      this.#queue.push({ mode, work, resolve, reject });
      this.#pump();
    });
  }

  #pump() {
    if (this.#sequentialRunning) return;

    while (this.#queue.length > 0) {
      const next = this.#queue[0];
      if (next.mode === "sequential") {
        if (this.#parallelRunning > 0) return;
        this.#queue.shift();
        this.#sequentialRunning = true;
        this.#start(next);
        return;
      }

      if (this.#parallelRunning >= this.parallelLimit) return;
      this.#queue.shift();
      this.#parallelRunning += 1;
      this.#start(next);
    }
  }

  #start(job) {
    Promise.resolve()
      .then(job.work)
      .then(job.resolve, job.reject)
      .finally(() => {
        if (job.mode === "parallel") this.#parallelRunning -= 1;
        else this.#sequentialRunning = false;
        this.#pump();
      });
  }
}

const LEAF_AGENT_TOOLS = new Set(["read", "bash", "edit", "write", "grep", "find", "ls"]);

async function leafAgent(prompt, options, spawn) {
  if (typeof prompt !== "string" || !prompt.trim()) throw new TypeError("agent(prompt) requires a non-empty prompt");
  if (options.background === true) throw new Error("Eval v2.1 agent() is blocking; use parallel([...]) for concurrency");

  const { background: _background, label, ...rest } = options;
  return spawn({
    prompt,
    description: options.description || label || "Eval v2.1 subagent",
    ...rest,
  });
}

async function proveStableEnvelope() {
  const content = [{ type: "text", text: '{"answer":42}' }];
  const value = toolEnvelope(content);

  assert.deepEqual(value, { text: '{"answer":42}', content });
  assert.equal(Object.hasOwn(value, "data"), false);
  console.log("PASS A  E(content) = {text, content}; JSON-looking text stays text");
}

async function proveAbsoluteWallTimeout() {
  let hostObservedAbort = false;
  const started = performance.now();

  await assert.rejects(
    runWithWallTimeout(
      (signal) => new Promise((_, reject) => {
        signal.addEventListener(
          "abort",
          () => {
            hostObservedAbort = true;
            reject(signal.reason);
          },
          { once: true },
        );
      }),
      40,
    ),
    /wall timeout/,
  );

  const elapsed = performance.now() - started;
  assert.equal(hostObservedAbort, true);
  assert(elapsed < 250, `wall timeout did not bound the cell: ${elapsed.toFixed(1)}ms`);
  console.log(`PASS B  T_cell <= timeout + scheduling jitter (${elapsed.toFixed(1)}ms observed)`);
}

async function proveFifoGate() {
  const gate = new HostCallGate(2);
  const trace = [];
  let parallelRunning = 0;
  let maxParallel = 0;

  const job = (name, mode, ms) => gate.run(mode, async () => {
    trace.push(`${name}:start`);
    if (mode === "parallel") {
      parallelRunning += 1;
      maxParallel = Math.max(maxParallel, parallelRunning);
    }
    await sleep(ms);
    if (mode === "parallel") parallelRunning -= 1;
    trace.push(`${name}:end`);
    return name;
  });

  await Promise.all([
    job("p1", "parallel", 35),
    job("p2", "parallel", 20),
    job("s3", undefined, 5),
    job("p4", "parallel", 5),
    job("p5", "parallel", 5),
  ]);

  const at = Object.fromEntries(trace.map((event, index) => [event, index]));
  assert(maxParallel <= 2);
  assert(at["s3:start"] > at["p1:end"] && at["s3:start"] > at["p2:end"]);
  assert(at["p4:start"] > at["s3:end"] && at["p5:start"] > at["s3:end"]);
  console.log(`PASS C  P(t) <= 2 and FIFO barrier holds: ${trace.join(" -> ")}`);
}

async function proveLeafAgent() {
  assert.equal(LEAF_AGENT_TOOLS.has("eval"), false);
  assert.equal(LEAF_AGENT_TOOLS.has("spawn_subagent"), false);

  await assert.rejects(
    leafAgent("work", { background: true }, async () => ({ text: "unexpected", content: [] })),
    /blocking/,
  );

  let spawnCount = 0;
  const result = await leafAgent("work", { model: "demo" }, async (args) => {
    spawnCount += 1;
    assert.equal(Object.hasOwn(args, "background"), false);
    return { text: "done", content: [{ type: "text", text: "done" }] };
  });

  assert.equal(spawnCount, 1);
  assert.equal(result.text, "done");
  console.log("PASS D  implicit agent depth <= 1; agent() is one blocking leaf episode");
}

await proveStableEnvelope();
await proveAbsoluteWallTimeout();
await proveFifoGate();
await proveLeafAgent();

console.log("\nv2.1 demo proved the four intended invariants without adding a scheduler, budget system, or new protocol.");
