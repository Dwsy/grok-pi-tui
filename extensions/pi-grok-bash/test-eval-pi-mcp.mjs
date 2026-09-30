import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createRequire } from "node:module";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { promisify } from "node:util";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const fromPi = createRequire(path.join(homedir(), ".pi", "agent", "npm", "package.json"));
const fromSource = createRequire(path.join(root, "pi-main", "package.json"));
const esbuild = fromSource("esbuild");
const execFileAsync = promisify(execFile);
const { Client } = fromPi("@modelcontextprotocol/sdk/client/index.js");
const { StreamableHTTPClientTransport } = fromPi("@modelcontextprotocol/sdk/client/streamableHttp.js");

const dir = await mkdtemp(path.join(tmpdir(), "eval-pi-mcp-test-"));
const oldHome = process.env.GROK_HOME;
process.env.GROK_HOME = dir;
try {
  await writeFile(path.join(dir, "package.json"), '{"private":true,"type":"module"}\n');
  await esbuild.build({
    entryPoints: [
      path.join(root, "extensions/pi-grok-bash/eval-pi-mcp.ts"),
      path.join(root, "extensions/pi-grok-bash/eval-token-count.ts"),
      path.join(root, "extensions/pi-grok-bash/eval.ts"),
    ],
    outdir: path.join(dir, "bundle"),
    entryNames: "[name]",
    bundle: true, platform: "node", format: "esm", target: "node22", logLevel: "silent",
    nodePaths: [
      path.join(homedir(), ".pi", "agent", "npm", "node_modules"),
      path.join(root, "pi-main", "node_modules"),
    ],
    banner: { js: 'import { createRequire as __createRequire } from "node:module"; const require = __createRequire(import.meta.url);' },
  });
  const { startEvalPiMcp, reserveEvalMcpBindingId } = await import(pathToFileURL(path.join(dir, "bundle/eval-pi-mcp.js")).href);
  const { EvalTokenCounter } = await import(pathToFileURL(path.join(dir, "bundle/eval-token-count.js")).href);
  const { PersistentEvalKernel } = await import(pathToFileURL(path.join(dir, "bundle/eval.js")).href);
  const png = Buffer.from("test-png").toString("base64");
  const imageHostCall = async (call) => {
    assert.equal(call.method, "tool");
    assert.equal(call.tool, "read");
    return {
      text: "read image fixture",
      content: [{ type: "text", text: "read image fixture" }, { type: "image", mimeType: "image/png", data: png }],
    };
  };
  const imageKernel = new PersistentEvalKernel("js", "v2", imageHostCall);
  const imageToolCatalog = [{
    name: "read", description: "Read file and image",
    schema: { type: "object", properties: { path: { type: "string" } } },
    executionMode: "parallel",
  }];
  const runRealRead = async () => imageKernel.execute(
    'const readResult = await tool.read({path:"read.png"}); console.log(readResult.text);',
    root, 12, undefined, false, imageToolCatalog,
  );
  const localRead = await runRealRead();
  assert.equal(localRead.images?.[0]?.mimeType, "image/png");
  assert.equal(localRead.images?.[0]?.data, png);
  console.log("PASS: real Eval v2 kernel captures nested read-image blocks without an explicit display(image) call");

  const tokenizer = await EvalTokenCounter.create("mock-tokenizer.json", () => ({
    Tokenizer: { fromFile: () => ({
      async encode(value) {
        const parts = value.trim() ? value.trim().split(/\s+/) : [];
        return { getIds: () => parts.map((_, i) => i + 1) };
      },
    }) },
  }));
  assert.deepEqual(
    [tokenizer.ready, ...(Object.values(await tokenizer.count("one two", "three four five")).slice(3, 6))],
    [true, 2, 3, 5],
    "tokenizers Encoding.getIds() drives input/output/total accounting",
  );
  const unavailable = await EvalTokenCounter.create("/nonexistent/model.json", () => undefined);
  const unavailableUsage = await unavailable.count("x", "y");
  assert.equal(unavailableUsage.status, "unavailable");
  assert.equal(unavailableUsage.input_tokens, null);
  console.log("PASS: tokenizers adapter counts exact mock tokenizer IDs; missing model never reports fabricated counts");

  const ids = await Promise.all(Array.from({ length: 24 }, () => reserveEvalMcpBindingId(path.join(dir, "bindings"))));
  assert.equal(new Set(ids).size, 24);
  const saved = JSON.parse(await readFile(path.join(dir, "bindings/issued-ids.json"), "utf8"));
  assert.equal(saved.length, 24);
  const reserveScript = `import { reserveEvalMcpBindingId } from ${JSON.stringify(pathToFileURL(path.join(dir, "bundle/eval-pi-mcp.js")).href)}; process.stdout.write(await reserveEvalMcpBindingId(${JSON.stringify(path.join(dir, "bindings"))}));`;
  const childReservations = await Promise.all(Array.from({ length: 6 }, () =>
    execFileAsync(process.execPath, ["--no-warnings", "--input-type=module", "-e", reserveScript])));
  const childIds = childReservations.map(({ stdout }) => stdout.trim());
  assert.equal(new Set([...ids, ...childIds]).size, 30);
  assert.equal(JSON.parse(await readFile(path.join(dir, "bindings/issued-ids.json"), "utf8")).length, 30);
  console.log("PASS: exclusive file lock reserves 24 concurrent in-process and 6 cross-process distinct, durable binding IDs");

  let called = [];
  const branch = [
    { type: "message", id: "x1", message: { role: "user", content: [{ type: "text", text: "What is the result?" }] } },
    { type: "message", id: "x2", message: { role: "assistant", content: [{ type: "text", text: "Use eval." }] } },
  ];
  const notices = [];
  const context = {
    cwd: root, model: { provider: "test", id: "fake" },
    sessionManager: { getBranch: () => branch, getSessionId: () => "live-session" },
  };
  const options = {
    context, languages: ["js"], description: "Real Eval v2 usage", guidelines: ["Use tools"],
    catalog: () => [{ name: "read", description: "Read a file", schema: { type: "object" } }],
    skills: () => [{ name: "research", description: "Research", filePath: "SKILL.md" }],
    execute: async (params) => {
      called.push(params);
      if (params.code === "image") {
        const imageFromRead = await runRealRead();
        return { content: [
          { type: "text", text: imageFromRead.output },
          ...imageFromRead.images,
        ] };
      }
      return { content: [{ type: "text", text: `Eval: ${params.code}` }] };
    },
    notify: (notice) => notices.push(notice),
  };
  const server = await startEvalPiMcp(options);
  assert.match(notices.join("\n"), /eval-pi-mcp binding ID:/);
  assert.match(notices.join("\n"), /MCP URL \(secret\):/);
  assert.ok(server.url.startsWith("http://127.0.0.1:"));
  const bindingFile = path.join(dir, "eval-pi-mcp", "binding.json");
  const persisted = JSON.parse(await readFile(bindingFile, "utf8"));
  assert.equal(persisted.bindingId, server.bindingId);
  assert.equal(persisted.url, server.url);
  assert.equal(persisted.pid, process.pid);
  assert.match(notices.join("\n"), /Recoverable: .+binding\.json/);
  const client = new Client({ name: "external-agent-test", version: "1.0" }, { capabilities: {} });
  try {
    await client.connect(new StreamableHTTPClientTransport(new URL(server.url)));
    const tools = await client.listTools();
    assert.deepEqual(tools.tools.map((tool) => tool.name).sort(), ["execute_eval", "get_context", "get_desc"]);
    const desc = await client.callTool({ name: "get_desc", arguments: { binding_id: server.bindingId } });
    const descValue = JSON.parse(desc.content[0].text);
    assert.equal(descValue.eval_description, "Real Eval v2 usage");
    assert.equal(descValue.registered_pi_tools[0].name, "read");
    assert.ok(desc.structuredContent?.token_usage);
    const ctx = await client.callTool({ name: "get_context", arguments: { binding_id: server.bindingId } });
    const snapshot = JSON.parse(ctx.content[0].text);
    assert.equal(snapshot.session_id, "live-session");
    assert.equal(snapshot.entries.length, 2);
    branch.push({ type: "message", id: "image-context", message: { role: "user", content: [
      { type: "image", mimeType: "image/png", data: "AQID".repeat(100_000) },
      { type: "text", text: "most recent visible user message" },
    ] } });
    const compressed = await client.callTool({ name: "get_context", arguments: { binding_id: server.bindingId } });
    const compact = JSON.parse(compressed.content[0].text);
    assert.ok(compressed.content[0].text.length < 120_000, "raw session image data must never flood context");
    assert.ok(!compressed.content[0].text.includes("AQIDAQIDAQID"), "image base64 must be omitted");
    assert.equal(compact.entries.at(-1).message.content[1].text, "most recent visible user message");
    branch.pop();
    const wrong = await client.callTool({ name: "get_context", arguments: { binding_id: ids[0] } });
    assert.equal(wrong.isError, true);
    assert.match(JSON.stringify(wrong.content), /Invalid or expired/);

    const evalResult = await client.callTool({
      name: "execute_eval", arguments: { binding_id: server.bindingId, language: "js", code: "40 + 2" },
    });
    assert.equal(evalResult.isError ?? false, false);
    assert.equal(evalResult.content[0].text, "Eval: 40 + 2");
    assert.equal(called[0].code, "40 + 2");
    assert.ok(evalResult.structuredContent?.token_usage);
    const disallowed = await client.callTool({
      name: "execute_eval", arguments: { binding_id: server.bindingId, language: "py", code: "40+2" },
    });
    assert.equal(disallowed.isError, true);
    const pic = await client.callTool({
      name: "execute_eval", arguments: { binding_id: server.bindingId, language: "js", code: "image" },
    });
    const link = pic.content.find((item) => item.type === "resource_link");
    assert.ok(link, "Pi read-display image becomes an MCP resource_link");
    assert.equal(link.mimeType, "image/png");
    const resource = await client.readResource({ uri: link.uri });
    assert.equal(resource.contents[0].blob, Buffer.from("test-png").toString("base64"));
    const resources = await client.listResources();
    assert.ok(resources.resources.some((item) => item.uri === link.uri), "image must be enumerable by the official SDK");

    const bare = new URL(server.url);
    bare.search = "";
    const unauthorized = await fetch(bare, { method: "POST" });
    assert.equal(unauthorized.status, 401);
    const hostile = await fetch(server.url, { method: "POST", headers: { origin: "https://attacker.invalid" } });
    assert.equal(hostile.status, 403);
    console.log("PASS: official MCP SDK tools/list/get_context/get_desc/execute_eval, binding checks, token usage, read image resources, URL auth and origin guard");
  } finally {
    await client.close();
    await server.close();
    imageKernel.close();
  }
  const again = await startEvalPiMcp(options);
  assert.notEqual(again.bindingId, server.bindingId);
  assert.notEqual(again.url, server.url);
  await again.close();
  await assert.rejects(readFile(bindingFile, "utf8"), /ENOENT/);
  console.log("PASS: session recreation rotates both binding ID and URL secret and clears the persisted binding record");
  let undisclosedUrl;
  await assert.rejects(startEvalPiMcp({
    ...options,
    notify(message) {
      undisclosedUrl = message.match(/MCP URL \(secret\): (http[^\s]+)/)?.[1];
      throw new Error("notification surface unavailable");
    },
  }), /notification surface unavailable/);
  assert.ok(undisclosedUrl);
  await assert.rejects(fetch(undisclosedUrl), /fetch failed/i);
  await assert.rejects(readFile(bindingFile, "utf8"), /ENOENT/);
  console.log("PASS: notification failure closes an otherwise unreachable, unauthenticated-by-owner binding");
} finally {
  if (oldHome === undefined) delete process.env.GROK_HOME;
  else process.env.GROK_HOME = oldHome;
  await rm(dir, { recursive: true, force: true });
}
