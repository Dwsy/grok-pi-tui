import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { loadSkills, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import piGrokSubagents from "./index.ts";

test("skill discovery follows the enabled V1/V2 tool surface and loads valid skills", async () => {
  const previousEnabled = process.env.PI_GROK_SUBAGENTS;
  const previousV2 = process.env.PI_GROK_SUBAGENTS_V2;
  const previousSocket = process.env.PI_GROK_SUBAGENT_SOCKET;
  const temporary = mkdtempSync(join(tmpdir(), "pi-skill-"));
  const sockets: Socket[] = [];
  const server = createServer((socket) => { sockets.push(socket); });
  const endpoint = join(temporary, "bridge.sock");
  await new Promise<void>((resolve) => server.listen(endpoint, resolve));
  process.env.PI_GROK_SUBAGENT_SOCKET = endpoint;
  try {
    for (const [enabled, v2, expected] of [
      ["0", "1", []],
      ["1", "0", ["multi-agent-proactive"]],
      ["1", "1", ["multi-agent-proactive", "multitask"]],
    ] as const) {
      process.env.PI_GROK_SUBAGENTS = enabled;
      process.env.PI_GROK_SUBAGENTS_V2 = v2;
      const handlers = new Map<string, (...args: any[]) => any>();
      const tools: string[] = [];
      const pi = {
        on: (event: string, handler: (...args: any[]) => any) => { handlers.set(event, handler); },
        registerTool: (tool: { name: string }) => { tools.push(tool.name); },
        registerCommand: () => undefined,
        events: { emit: () => undefined },
      } as unknown as ExtensionAPI;
      const connected = enabled === "1" ? once(server, "connection") : Promise.resolve();
      piGrokSubagents(pi);
      await connected;
      const discovered = await handlers.get("resources_discover")?.({}, {});
      const paths: string[] = discovered?.skillPaths ?? [];
      assert.deepEqual(paths.map((path) => basename(dirname(path))), [...expected]);
      const loaded = loadSkills({
        cwd: process.cwd(),
        agentDir: dirname(fileURLToPath(import.meta.url)),
        skillPaths: paths,
        includeDefaults: false,
      });
      assert.deepEqual(loaded.skills.map((skill) => skill.name).sort(), [...expected].sort());
      assert.deepEqual(loaded.diagnostics, []);
      assert.equal(tools.includes("spawn_team_agent"), enabled === "1" && v2 === "1");
      assert.equal(tools.includes("spawn_subagent"), enabled === "1" && v2 === "0");
    }
  } finally {
    if (previousEnabled === undefined) delete process.env.PI_GROK_SUBAGENTS;
    else process.env.PI_GROK_SUBAGENTS = previousEnabled;
    if (previousV2 === undefined) delete process.env.PI_GROK_SUBAGENTS_V2;
    else process.env.PI_GROK_SUBAGENTS_V2 = previousV2;
    if (previousSocket === undefined) delete process.env.PI_GROK_SUBAGENT_SOCKET;
    else process.env.PI_GROK_SUBAGENT_SOCKET = previousSocket;
    for (const socket of sockets) socket.destroy();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    rmSync(temporary, { recursive: true, force: true });
  }
});
