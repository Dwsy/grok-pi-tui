import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createRequire } from "node:module";
import { homedir, tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const fromPi = createRequire(path.join(homedir(), ".pi", "agent", "npm", "package.json"));
const fromSource = createRequire(path.join(root, "pi-main", "package.json"));
const esbuild = fromSource("esbuild");
const { Client } = fromPi("@modelcontextprotocol/sdk/client/index.js");
const { StreamableHTTPClientTransport } = fromPi("@modelcontextprotocol/sdk/client/streamableHttp.js");

const dir = await mkdtemp(path.join(tmpdir(), "mcpeval-daemon-test-"));
let daemon;
let pi;
let client;
try {
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
  const mod = await import(pathToFileURL(path.join(dir, "bundle/eval-pi-mcp.js")).href);
  const { startMcpevalDaemon, registerEvalPi, discoverMcpevalDaemon } = mod;
  const branch = [
    { type: "message", id: "u1", message: { role: "user", content: [{ type: "text", text: "hello" }] } },
  ];
  const context = {
    cwd: root, model: { provider: "test", id: "fake" },
    sessionManager: { getBranch: () => branch, getSessionId: () => "daemon-test-session" },
  };
  const notices = [];
  daemon = await startMcpevalDaemon({ root: path.join(dir, "daemon"), port: 0 });
  assert.equal(daemon.descriptor().protocol, "mcpeval/1");
  assert.equal((await discoverMcpevalDaemon(path.join(dir, "daemon"))).url, daemon.url);
  const daemonUrl = new URL(daemon.url);
  const dashboardUrl = new URL("/", daemonUrl);
  dashboardUrl.search = daemonUrl.search;
  const dashboard = await fetch(dashboardUrl);
  assert.equal(dashboard.status, 200);
  assert.match(await dashboard.text(), /mcpeval daemon/);
  const statusResponse = await fetch(new URL("/api/status" + daemonUrl.search, daemonUrl));
  assert.equal(statusResponse.status, 200);
  assert.equal((await statusResponse.json()).daemon.protocol, "mcpeval/1");
  const oauthMetadataResponse = await fetch(new URL("/.well-known/oauth-authorization-server", daemonUrl));
  assert.equal(oauthMetadataResponse.status, 200);
  assert.deepEqual((await oauthMetadataResponse.json()).code_challenge_methods_supported, ["S256"]);
  pi = await registerEvalPi({
    root: path.join(dir, "daemon"), context, languages: ["js"],
    description: "daemon Eval", guidelines: ["Use Eval"],
    catalog: () => [{ name: "read", description: "Read files", schema: { type: "object" } }],
    skills: () => [{ name: "research", description: "Research", filePath: "SKILL.md" }],
    execute: async (params, signal) => {
      if (signal.aborted) throw new Error("cancelled");
      return { content: [{ type: "text", text: "executed: " + params.code }] };
    },
    notify: (notice) => notices.push(notice),
  });
  assert.match(notices[0], /mcpeval binding ID:/);
  client = new Client({ name: "mcpeval-daemon-test", version: "1.0" }, { capabilities: {} });
  try {
    await client.connect(new StreamableHTTPClientTransport(new URL(pi.url)));
    const listed = await client.listTools();
    assert.deepEqual(listed.tools.map((tool) => tool.name).sort(),
      ["discover", "events_poll", "events_subscribe", "events_unsubscribe", "execute_eval", "get_context", "get_desc", "search"]);
    const discovered = await client.callTool({ name: "discover", arguments: {} });
    const discoverValue = JSON.parse(discovered.content[0].text);
    assert.equal(discoverValue.registrations[0].binding_id, pi.bindingId);
    const searched = await client.callTool({ name: "search", arguments: { query: "read" } });
    assert.equal(JSON.parse(searched.content[0].text)[0].name, "read");
    const desc = await client.callTool({ name: "get_desc", arguments: { binding_id: pi.bindingId } });
    assert.match(desc.content[0].text, /daemon Eval/);
    const ctx = await client.callTool({ name: "get_context", arguments: { binding_id: pi.bindingId } });
    assert.match(ctx.content[0].text, /daemon-test-session/);
    const executed = await client.callTool({
      name: "execute_eval", arguments: { binding_id: pi.bindingId, language: "js", code: "40 + 2" },
    });
    assert.match(executed.content[0].text, /executed: 40 \+ 2/);

    const subscribed = await client.callTool({
      name: "events_subscribe",
      arguments: { binding_id: pi.bindingId, filter: { nativeType: "runtime.changed" } },
    });
    const subscriptionId = JSON.parse(subscribed.content[0].text).subscription_id;
    daemon.publish(pi.bindingId, { eventId: "duplicate-event", nativeType: "runtime.changed", threadId: "thread-1", native: { value: 1 } });
    daemon.publish(pi.bindingId, { eventId: "duplicate-event", nativeType: "runtime.changed", threadId: "thread-1", native: { value: 99 } });
    daemon.publish(pi.bindingId, { nativeType: "ignored", native: { value: 2 } });
    const polled = await client.callTool({ name: "events_poll", arguments: { subscription_id: subscriptionId } });
    const events = JSON.parse(polled.content[0].text).events;
    assert.equal(events.length, 1);
    assert.equal(events[0].nativeType, "runtime.changed");
    assert.equal(events[0].native.value, 1);
    assert.equal(events[0].bindingId, pi.bindingId);
    assert.equal(typeof events[0].epoch, "string");
    await client.callTool({ name: "events_unsubscribe", arguments: { subscription_id: subscriptionId } });

    const bounded = await client.callTool({
      name: "events_subscribe",
      arguments: { binding_id: pi.bindingId, filter: { nativeType: "runtime.changed" } },
    });
    const boundedId = JSON.parse(bounded.content[0].text).subscription_id;
    for (let i = 0; i < 70; i++) daemon.publish(pi.bindingId, { eventId: `overflow-${i}`, nativeType: "runtime.changed", native: { value: i } });
    const overflow = await client.callTool({ name: "events_poll", arguments: { subscription_id: boundedId } });
    const overflowEvents = JSON.parse(overflow.content[0].text).events;
    assert.equal(overflowEvents[0].nativeType, "agenvo.resync_required");
    await client.callTool({ name: "events_unsubscribe", arguments: { subscription_id: boundedId } });
  } finally {
    await client.close().catch(() => {});
  }
  await pi.close();
  assert.equal(daemon.discover().length, 0);
  const afterClose = await discoverMcpevalDaemon(path.join(dir, "daemon"));
  assert.equal(afterClose.url, daemon.url);
  await daemon.close();
  await assert.rejects(readFile(path.join(dir, "daemon", "daemon.json"), "utf8"), /ENOENT/);
  console.log("PASS: resident mcpeval daemon registration/discovery, request forwarding and bounded event polling");
} finally {
  await client?.close().catch(() => {});
  await pi?.close().catch(() => {});
  await daemon?.close().catch(() => {});
  await rm(dir, { recursive: true, force: true });
}
