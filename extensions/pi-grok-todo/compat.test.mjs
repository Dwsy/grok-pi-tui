import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { createJiti } = require("../../pi-main/node_modules/jiti/lib/jiti.mjs");

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const jiti = createJiti(import.meta.url, {
  alias: { typebox: path.join(repoRoot, "pi-main/node_modules/typebox/build/index.mjs") },
});

function branchWith(...details) {
  return details.map((details) => ({
    type: "message",
    message: { role: "toolResult", toolName: "todo", details },
  }));
}

// A status-only merged write: leaves existing items untouched while forcing
// the tool to replay the branch and persist a fresh own-version snapshot.
const PROBE = Object.freeze([{ id: "zz-probe", content: "Probe item", status: "pending" }]);

function harness(branch) {
  let tool;
  const pi = {
    registerTool(definition) {
      tool = definition;
    },
    registerCommand() {},
    events: { on() {} },
    on() {},
  };
  const ctx = { sessionManager: { getBranch: () => branch } };
  return {
    async execute(params) {
      return tool.execute("call-1", params, undefined, undefined, ctx);
    },
    pi,
  };
}

test("v2 → v1: latest v2 snapshot migrates into string-id v1 items", async () => {
  const mod = await jiti.import("./v1.ts");
  const h = harness(
    branchWith({
      version: 2,
      action: "update",
      params: {},
      nextId: 4,
      tasks: [
        { id: 1, subject: "Explore codebase", status: "completed" },
        { id: 2, subject: "Write tests", status: "in_progress" },
        { id: 3, subject: "Gone", status: "deleted" },
        { id: 4, subject: "Polish docs", status: "pending" },
      ],
    }),
  );
  mod.registerV1(h.pi);
  const result = await h.execute({ todos: PROBE });
  assert.match(result.content[0].text, /- \[completed\] 1: Explore codebase/);
  assert.match(result.content[0].text, /- \[in_progress\] 2: Write tests/);
  assert.match(result.content[0].text, /- \[pending\] 4: Polish docs/);
  assert.doesNotMatch(result.content[0].text, /Gone/);
  assert.match(result.content[0].text, /Probe item/);
  // The write lands as an own-version snapshot so later calls replay natively.
  assert.equal(result.details.version, 1);
  assert.equal(result.details.todos.length, 4);
});

test("v1 → v2: first write lands migrated state with numeric ids", async () => {
  const mod = await jiti.import("./v2.ts");
  const h = harness(
    branchWith({
      version: 1,
      todos: [
        { id: "a", content: "Task A", status: "pending" },
        { id: "b", content: "Task B", status: "cancelled" },
      ],
      tasks: [],
      nextId: 0,
    }),
  );
  mod.registerV2(h.pi);
  const result = await h.execute({ action: "create", subject: "Task C" });
  assert.deepEqual(
    result.details.tasks.map((t) => [t.id, t.subject, t.status]),
    [
      [1, "Task A", "pending"],
      [2, "Task B", "deleted"],
      [3, "Task C", "pending"],
    ],
  );
  assert.equal(result.details.nextId, 4);
  assert.equal(result.details.version, 2);
});

test("latest snapshot wins across repeated switches (no stale own-kind replay)", async () => {
  const mod = await jiti.import("./v1.ts");
  const h = harness(
    branchWith(
      { version: 1, todos: [{ id: "old", content: "Stale v1 task", status: "pending" }], tasks: [], nextId: 0 },
      {
        version: 2,
        action: "create",
        params: {},
        nextId: 3,
        tasks: [
          { id: 1, subject: "Fresh A", status: "in_progress" },
          { id: 2, subject: "Fresh B", status: "pending" },
        ],
      },
    ),
  );
  mod.registerV1(h.pi);
  const result = await h.execute({ todos: PROBE });
  assert.match(result.content[0].text, /Fresh A/);
  assert.match(result.content[0].text, /Fresh B/);
  assert.doesNotMatch(result.content[0].text, /Stale v1 task/);
});

test("legacy unmarked snapshots still replay in both versions", async () => {
  const legacy = branchWith({
    action: "create",
    params: {},
    nextId: 8,
    tasks: [{ id: 7, subject: "Legacy task", status: "pending" }],
  });

  const v1mod = await jiti.import("./v1.ts");
  const h1 = harness(legacy);
  v1mod.registerV1(h1.pi);
  const r1 = await h1.execute({ todos: PROBE });
  assert.match(r1.content[0].text, /\[pending\] 7: Legacy task/);

  const v2mod = await jiti.import("./v2.ts");
  const h2 = harness(legacy);
  v2mod.registerV2(h2.pi);
  const r2 = await h2.execute({ action: "create", subject: "Probe item" });
  assert.deepEqual(
    r2.details.tasks.map((t) => [t.id, t.subject, t.status]),
    [
      [7, "Legacy task", "pending"],
      [8, "Probe item", "pending"],
    ],
  );
  assert.equal(r2.details.nextId, 9);
});
