import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const EXTENSION_PATH = fileURLToPath(new URL("./index.ts", import.meta.url));

function findTypeboxUrl() {
  const candidates = [
    process.env.PI_TEST_NODE_MODULES && join(process.env.PI_TEST_NODE_MODULES, "typebox/build/esm/index.mjs"),
    process.env.PI_TEST_NODE_MODULES && join(process.env.PI_TEST_NODE_MODULES, "typebox/build/cjs/index.js"),
    "/Users/dengwenyu/.pi/agent/install/releases/1.5.2/node_modules/typebox/build/esm/index.mjs",
    "/Users/dengwenyu/.pi/agent/install/releases/1.5.2/node_modules/typebox/build/cjs/index.js",
  ].filter(Boolean);
  for (const candidate of candidates) {
    if (existsSync(candidate)) return pathToFileURL(candidate).href;
  }
  const stub = `export const Type = {
    Object: (properties) => ({ type: "object", properties }),
    Optional: (schema) => ({ ...schema, optional: true }),
    String: (opts = {}) => ({ type: "string", ...opts }),
  };`;
  return `data:text/javascript;base64,${Buffer.from(stub, "utf8").toString("base64")}`;
}

const TYPEBOX_URL = findTypeboxUrl();

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier === "@sinclair/typebox") {
      return { url: TYPEBOX_URL, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
});

const { default: registerPlanMode } = await import("./index.ts");

function createHarness() {
  const dir = mkdtempSync(join(tmpdir(), "pi-plan-mode-test-"));
  const controlFile = join(dir, "control.json");
  const planFile = join(dir, "plans", "session.plan.md");
  const prevEnv = process.env.PI_GROK_PLAN_CONTROL;
  process.env.PI_GROK_PLAN_CONTROL = controlFile;

  const handlers = new Map();
  const tools = new Map();
  const pi = {
    on: (event, handler) => {
      handlers.set(event, handler);
    },
    registerTool: (def) => {
      tools.set(def.name, def);
    },
  };
  registerPlanMode(pi);

  const writeControl = (value) => {
    writeFileSync(controlFile, JSON.stringify(value), "utf8");
  };
  const responsePathFor = (toolCallId) => {
    const digest = createHash("sha256").update(toolCallId, "utf8").digest("hex");
    return join(dir, `plan-resp-${digest}.json`);
  };
  const writeResponse = (toolCallId, payload) => {
    writeFileSync(responsePathFor(toolCallId), JSON.stringify(payload), "utf8");
  };
  const cleanup = () => {
    if (prevEnv === undefined) delete process.env.PI_GROK_PLAN_CONTROL;
    else process.env.PI_GROK_PLAN_CONTROL = prevEnv;
    rmSync(dir, { recursive: true, force: true });
  };

  return {
    dir,
    controlFile,
    planFile,
    handlers,
    tools,
    writeControl,
    responsePathFor,
    writeResponse,
    cleanup,
  };
}

test("registers enter_plan_mode and exit_plan_mode with proactive prompt snippets and guidelines", () => {
  const h = createHarness();
  try {
    const enter = h.tools.get("enter_plan_mode");
    const exit = h.tools.get("exit_plan_mode");
    assert.ok(enter, "enter_plan_mode registered");
    assert.ok(exit, "exit_plan_mode registered");
    assert.ok(typeof enter.promptSnippet === "string" && enter.promptSnippet.includes("enter Plan Mode"));
    assert.ok(Array.isArray(enter.promptGuidelines) && enter.promptGuidelines.length >= 2);
    assert.ok(typeof exit.promptSnippet === "string" && exit.promptSnippet.includes("exit Plan Mode"));
    assert.ok(
      Array.isArray(exit.promptGuidelines) &&
        exit.promptGuidelines.some((line) => line.includes("same turn") && line.includes("exit_plan_mode")),
    );
  } finally {
    h.cleanup();
  }
});

test("tool_call gate blocks bash, non-plan edits, and mutating subagents while allowing plan file & read-only subagents", async () => {
  const h = createHarness();
  try {
    const gate = h.handlers.get("tool_call");
    assert.ok(gate);
    const ctx = { cwd: h.dir };

    h.writeControl({ active: false, planFilePath: h.planFile, syncResponses: true });
    assert.equal(await gate({ toolName: "bash", input: { command: "echo ok" } }, ctx), undefined);
    assert.equal(await gate({ toolName: "edit", input: { path: "src/main.ts" } }, ctx), undefined);
    assert.equal(await gate({ toolName: "spawn_team_agent", input: { task_name: "dev" } }, ctx), undefined);

    h.writeControl({ active: true, planFilePath: h.planFile, syncResponses: true });

    const bashRes = await gate({ toolName: "bash", input: { command: "ls" } }, ctx);
    assert.equal(bashRes.block, true);
    assert.match(bashRes.reason, /bash is disabled in plan mode/);

    const badEdit = await gate({ toolName: "edit", input: { path: "src/app.ts" } }, ctx);
    assert.equal(badEdit.block, true);
    assert.match(badEdit.reason, /only editable file is the plan file/);

    const badWrite = await gate({ toolName: "write", input: { path: join(h.dir, "other.md") } }, ctx);
    assert.equal(badWrite.block, true);

    assert.equal(await gate({ toolName: "write", input: { path: h.planFile } }, ctx), undefined);
    assert.equal(
      await gate({ toolName: "edit", input: { path: "plans/session.plan.md" } }, ctx),
      undefined,
    );

    for (const spawnTool of ["spawn_team_agent", "spawn_team", "spawn_subagent"]) {
      const blockedDefault = await gate({ toolName: spawnTool, input: { task_name: "worker" } }, ctx);
      assert.equal(blockedDefault.block, true, `${spawnTool} without capability_mode must be blocked`);
      assert.match(blockedDefault.reason, /capability_mode: "read-only"/);

      const blockedAll = await gate(
        { toolName: spawnTool, input: { task_name: "worker", capability_mode: "all" } },
        ctx,
      );
      assert.equal(blockedAll.block, true, `${spawnTool} with capability_mode=all must be blocked`);

      const allowedReadOnly = await gate(
        { toolName: spawnTool, input: { task_name: "recon", capability_mode: "read-only" } },
        ctx,
      );
      assert.equal(allowedReadOnly, undefined, `${spawnTool} with capability_mode=read-only must be allowed`);

      const blockedForkDefault = await gate(
        { toolName: spawnTool, input: { task_name: "forked_dev", fork: true } },
        ctx,
      );
      assert.equal(blockedForkDefault.block, true, `${spawnTool} with fork=true and no read-only must be blocked`);

      const allowedForkReadOnly = await gate(
        { toolName: spawnTool, input: { task_name: "forked_recon", fork: true, capability_mode: "read-only" } },
        ctx,
      );
      assert.equal(allowedForkReadOnly, undefined, `${spawnTool} with fork=true and capability_mode=read-only must be allowed`);
    }
  } finally {
    h.cleanup();
  }
});

test("enter_plan_mode waits for sync adapter response, matches SHA-256 contract, cleans up, and respects abort", async () => {
  const h = createHarness();
  try {
    h.writeControl({ active: false, planFilePath: h.planFile, syncResponses: true });
    assert.equal(
      h.responsePathFor("call-1"),
      join(
        h.dir,
        "plan-resp-5d7963c4f471e142f5a72214a9666fb164718f9ba1066a7862ac1c5041887940.json",
      ),
    );

    const enter = h.tools.get("enter_plan_mode");
    const pending = enter.execute("call-1", { reason: "multi-file refactor" });
    setTimeout(() => {
      h.writeControl({ active: true, planFilePath: h.planFile, syncResponses: true });
      h.writeResponse("call-1", {
        outcome: "entered",
        alreadyActive: false,
        reentry: false,
        seedStatus: "empty",
        planFilePath: h.planFile,
        message: "Entered plan mode. Write your plan.",
      });
    }, 35);

    const result = await pending;
    assert.equal(result.content[0].text, "Entered plan mode. Write your plan.");
    assert.equal(result.details.outcome, "entered");
    assert.equal(result.details.planFilePath, h.planFile);
    assert.equal(existsSync(h.responsePathFor("call-1")), false, "consumed response file must be removed");

    // Abort while waiting must reject cleanly without hanging.
    const ac = new AbortController();
    const abortedCall = enter.execute("call-abort", { reason: "test" }, ac.signal);
    setTimeout(() => ac.abort(), 30);
    await assert.rejects(abortedCall, /aborted/);
  } finally {
    h.cleanup();
  }
});

test("exit_plan_mode stays idempotent when inactive and faithfully returns approved, abandoned, and cancelled verdicts", async () => {
  const h = createHarness();
  try {
    const exit = h.tools.get("exit_plan_mode");

    // Inactive idempotency
    h.writeControl({ active: false, planFilePath: h.planFile, syncResponses: true });
    const inactive = await exit.execute("call-inactive", {});
    assert.match(inactive.content[0].text, /Plan mode is not active/);
    assert.equal(inactive.details.outcome, "inactive");

    // Approved
    h.writeControl({ active: true, planFilePath: h.planFile, syncResponses: true });
    const approvedPromise = exit.execute("call-approve", {});
    setTimeout(() => {
      h.writeControl({ active: false, planFilePath: h.planFile, syncResponses: true });
      h.writeResponse("call-approve", {
        outcome: "approved",
        planFilePath: h.planFile,
        message: "The user approved the plan. Plan mode is now OFF.",
      });
    }, 35);
    const approved = await approvedPromise;
    assert.equal(approved.details.outcome, "approved");
    assert.match(approved.content[0].text, /approved the plan/);

    // Abandoned
    h.writeControl({ active: true, planFilePath: h.planFile, syncResponses: true });
    const abandonedPromise = exit.execute("call-abandon", {});
    setTimeout(() => {
      h.writeControl({ active: false, planFilePath: h.planFile, syncResponses: true });
      h.writeResponse("call-abandon", {
        outcome: "abandoned",
        planFilePath: h.planFile,
        message: "The user chose to abandon the plan entirely. Do NOT implement.",
      });
    }, 35);
    const abandoned = await abandonedPromise;
    assert.equal(abandoned.details.outcome, "abandoned");
    assert.match(abandoned.content[0].text, /abandon the plan/);

    // Cancelled with feedback
    h.writeControl({ active: true, planFilePath: h.planFile, syncResponses: true });
    const cancelledPromise = exit.execute("call-cancel", {});
    setTimeout(() => {
      h.writeResponse("call-cancel", {
        outcome: "cancelled",
        planFilePath: h.planFile,
        message: "The user requested plan changes:\nAdd rollback notes.",
      });
    }, 35);
    const cancelled = await cancelledPromise;
    assert.equal(cancelled.details.outcome, "cancelled");
    assert.match(cancelled.content[0].text, /Add rollback notes/);
  } finally {
    h.cleanup();
  }
});

test("standalone fallback enters plan mode and persists inline plan when syncResponses is false", async () => {
  const h = createHarness();
  try {
    h.writeControl({ active: false, planFilePath: h.planFile });
    const enter = h.tools.get("enter_plan_mode");
    const exit = h.tools.get("exit_plan_mode");

    const entered = await enter.execute("standalone-1", { reason: "test" });
    assert.equal(entered.details.outcome, "entered");
    assert.equal(JSON.parse(readFileSync(h.controlFile, "utf8")).active, true);

    const exited = await exit.execute("standalone-2", { plan: "# Plan\n\n## Goal\nTest\n" });
    assert.equal(exited.content[0].text, "Plan submitted for user approval.");
    assert.equal(readFileSync(h.planFile, "utf8"), "# Plan\n\n## Goal\nTest\n");
  } finally {
    h.cleanup();
  }
});

test("real pi RPC process loads pi-grok-plan-mode without errors", async (t) => {
  const probe = await new Promise((resolveProbe) => {
    const child = spawn("pi", ["-ne", "--mode", "rpc", "--extension", EXTENSION_PATH], {
      cwd: dirname(EXTENSION_PATH),
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      resolveProbe({ timedOut: true, stdout, stderr, code: null });
    }, 8000);
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", (err) => {
      clearTimeout(timer);
      resolveProbe({ spawnError: err, stdout, stderr, code: null });
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolveProbe({ code, stdout, stderr, timedOut: false });
    });
    child.stdin.write(`${JSON.stringify({ type: "get_state" })}\n`);
    child.stdin.end();
  });

  if (probe.spawnError?.code === "ENOENT") {
    t.skip("pi binary not installed in PATH");
    return;
  }
  assert.equal(probe.timedOut, false, `pi RPC timed out: stderr=${probe.stderr}`);
  assert.equal(probe.code, 0, `pi RPC exited non-zero: stderr=${probe.stderr}`);
  assert.equal(probe.stderr.trim(), "", `unexpected pi RPC stderr: ${probe.stderr}`);
  assert.match(probe.stdout, /"command":"get_state","success":true/);
});
