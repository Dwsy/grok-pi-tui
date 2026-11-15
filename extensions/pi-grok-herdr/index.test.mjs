import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const sourcePath = new URL("./index.ts", import.meta.url);

async function importFresh(tempDir, suffix) {
  const source = await readFile(sourcePath, "utf8");
  const modulePath = path.join(tempDir, `herdr-${suffix}.mjs`);
  await writeFile(modulePath, source);
  return import(`${pathToFileURL(modulePath).href}?v=${Date.now()}-${Math.random()}`);
}

function harness() {
  const lifecycle = new Map();
  const events = new Map();
  return {
    lifecycle,
    events,
    pi: {
      on(name, handler) {
        lifecycle.set(name, handler);
      },
      events: {
        on(name, handler) {
          events.set(name, handler);
        },
      },
    },
  };
}

async function waitFor(predicate, message) {
  const deadline = Date.now() + 2000;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  assert.fail(message);
}

async function recordingServer(socketPath, respond = (request, socket) => {
  socket.end(JSON.stringify({ id: request.id, result: { type: "ok" } }) + "\n");
}) {
  const requests = [];
  const server = net.createServer((socket) => {
    let buffer = "";
    socket.setEncoding("utf8");
    socket.on("data", (chunk) => {
      buffer += chunk;
      const newline = buffer.indexOf("\n");
      if (newline < 0) return;
      const request = JSON.parse(buffer.slice(0, newline));
      requests.push(request);
      respond(request, socket, requests);
    });
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });
  return { requests, server };
}

test("is a silent no-op outside Herdr", { concurrency: false }, async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "pi-grok-herdr-noop-"));
  const saved = {
    env: process.env.HERDR_ENV,
    socket: process.env.HERDR_SOCKET_PATH,
    pane: process.env.HERDR_PANE_ID,
  };
  delete process.env.HERDR_ENV;
  delete process.env.HERDR_SOCKET_PATH;
  delete process.env.HERDR_PANE_ID;
  try {
    const { default: install } = await importFresh(tempDir, "noop");
    const h = harness();
    install(h.pi);
    assert.equal(h.lifecycle.size, 0);
    assert.equal(h.events.size, 0);
  } finally {
    if (saved.env === undefined) delete process.env.HERDR_ENV;
    else process.env.HERDR_ENV = saved.env;
    if (saved.socket === undefined) delete process.env.HERDR_SOCKET_PATH;
    else process.env.HERDR_SOCKET_PATH = saved.socket;
    if (saved.pane === undefined) delete process.env.HERDR_PANE_ID;
    else process.env.HERDR_PANE_ID = saved.pane;
    await rm(tempDir, { recursive: true, force: true });
  }
});

test("reports ordered session, working, blocked, and settled states", { concurrency: false }, async (t) => {
  if (process.platform === "win32") t.skip("Unix socket smoke test");

  const tempDir = await mkdtemp(path.join(os.tmpdir(), "pi-grok-herdr-smoke-"));
  const socketPath = path.join(tempDir, "herdr.sock");
  const { requests, server } = await recordingServer(socketPath);
  const saved = {
    env: process.env.HERDR_ENV,
    socket: process.env.HERDR_SOCKET_PATH,
    pane: process.env.HERDR_PANE_ID,
  };
  process.env.HERDR_ENV = "1";
  process.env.HERDR_SOCKET_PATH = socketPath;
  process.env.HERDR_PANE_ID = "w-test:p1";

  try {
    const { default: install } = await importFresh(tempDir, "smoke");
    const h = harness();
    install(h.pi);

    assert.deepEqual([...h.lifecycle.keys()].sort(), ["agent_settled", "agent_start", "session_start"]);
    assert.deepEqual([...h.events.keys()], ["herdr:blocked"]);

    let idle = true;
    const ctx = {
      hasUI: true,
      isIdle: () => idle,
      sessionManager: {
        getSessionFile: () => "/tmp/grok-pi-session.jsonl",
        getSessionId: () => "grok-pi-session",
      },
    };

    await h.lifecycle.get("session_start")({ reason: "startup" }, ctx);
    await waitFor(
      () => requests.filter((r) => r.method === "pane.report_agent").length === 1,
      "initial idle report",
    );
    assert.deepEqual(requests.slice(0, 2).map((r) => r.method), [
      "pane.report_agent_session",
      "pane.report_agent",
    ]);
    assert.equal(requests[0].params.session_start_source, "startup");
    assert.equal(requests[0].params.source, "herdr:pi");
    assert.equal(requests[1].params.state, "idle");
    assert.ok(requests[0].params.seq < requests[1].params.seq);

    idle = false;
    h.lifecycle.get("agent_start")({}, ctx);
    await waitFor(
      () => requests.some((r) => r.method === "pane.report_agent" && r.params.state === "working"),
      "working report",
    );

    h.events.get("herdr:blocked")({ active: true, label: "approval" });
    await waitFor(
      () => requests.some((r) => r.method === "pane.report_agent" && r.params.state === "blocked"),
      "blocked report",
    );

    idle = true;
    const beforeSettled = requests.length;
    h.lifecycle.get("agent_settled")({}, ctx);
    await new Promise((resolve) => setTimeout(resolve, 50));
    assert.equal(requests.length, beforeSettled, "blocked state must take precedence over settlement");

    h.events.get("herdr:blocked")({ active: false });
    await waitFor(
      () => requests.filter((r) => r.method === "pane.report_agent").at(-1)?.params.state === "idle",
      "settled idle report",
    );

    assert.equal(requests.some((r) => r.method === "pane.release_agent"), false);
    const states = requests
      .filter((r) => r.method === "pane.report_agent")
      .map((r) => r.params.state);
    assert.deepEqual(states, ["idle", "working", "blocked", "idle"]);
    assert.ok(requests.every((r, i) => i === 0 || r.params.seq > requests[i - 1].params.seq));
  } finally {
    await new Promise((resolve) => server.close(resolve));
    if (saved.env === undefined) delete process.env.HERDR_ENV;
    else process.env.HERDR_ENV = saved.env;
    if (saved.socket === undefined) delete process.env.HERDR_SOCKET_PATH;
    else process.env.HERDR_SOCKET_PATH = saved.socket;
    if (saved.pane === undefined) delete process.env.HERDR_PANE_ID;
    else process.env.HERDR_PANE_ID = saved.pane;
    await rm(tempDir, { recursive: true, force: true });
  }
});

test("serializes agent_start session and state, including session replacement", { concurrency: false }, async (t) => {
  if (process.platform === "win32") t.skip("Unix socket smoke test");

  const tempDir = await mkdtemp(path.join(os.tmpdir(), "pi-grok-herdr-order-"));
  const socketPath = path.join(tempDir, "herdr.sock");
  const { requests, server } = await recordingServer(socketPath, (request, socket, all) => {
    const respond = () => socket.end(JSON.stringify({
      id: request.id, result: { type: "ok" },
    }) + "\n");
    if (all.length === 1) setTimeout(respond, 80);
    else respond();
  });
  const saved = {
    env: process.env.HERDR_ENV,
    socket: process.env.HERDR_SOCKET_PATH,
    pane: process.env.HERDR_PANE_ID,
  };
  process.env.HERDR_ENV = "1";
  process.env.HERDR_SOCKET_PATH = socketPath;
  process.env.HERDR_PANE_ID = "w-test:p2";

  try {
    const { default: install } = await importFresh(tempDir, "order");
    const h = harness();
    install(h.pi);
    let id = "session-a";
    let idle = true;
    const ctx = {
      hasUI: true,
      isIdle: () => idle,
      sessionManager: {
        getSessionFile: () => undefined,
        getSessionId: () => id,
      },
    };

    // Fire callbacks synchronously to expose any session/state send race.
    h.lifecycle.get("session_start")({ reason: "startup" }, ctx);
    idle = false;
    h.lifecycle.get("agent_start")({}, ctx);
    await waitFor(() => requests.length === 1, "first session report dispatched");
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.equal(requests.length, 1, "state must wait for session acknowledgment");
    await waitFor(() => requests.length >= 4, "initial state and agent_start reports");
    assert.deepEqual(requests.slice(0, 4).map((r) => r.method), [
      "pane.report_agent_session", "pane.report_agent",
      "pane.report_agent_session", "pane.report_agent",
    ]);
    assert.deepEqual(requests.slice(0, 4).map((r) => r.params.agent_session_id), [
      "session-a", "session-a", "session-a", "session-a",
    ]);

    id = "session-b";
    idle = true;
    h.lifecycle.get("session_start")({ reason: "resume" }, ctx);
    await waitFor(() => requests.length >= 6, "replacement session and idle reports");
    assert.deepEqual(requests.slice(4, 6).map((r) => r.method), [
      "pane.report_agent_session", "pane.report_agent",
    ]);
    assert.equal(requests[4].params.session_start_source, "resume");
    assert.deepEqual(requests.slice(4, 6).map((r) => r.params.agent_session_id), [
      "session-b", "session-b",
    ]);
    assert.ok(requests.every((r, i) => i === 0 || r.params.seq > requests[i - 1].params.seq));
  } finally {
    await new Promise((resolve) => server.close(resolve));
    if (saved.env === undefined) delete process.env.HERDR_ENV;
    else process.env.HERDR_ENV = saved.env;
    if (saved.socket === undefined) delete process.env.HERDR_SOCKET_PATH;
    else process.env.HERDR_SOCKET_PATH = saved.socket;
    if (saved.pane === undefined) delete process.env.HERDR_PANE_ID;
    else process.env.HERDR_PANE_ID = saved.pane;
    await rm(tempDir, { recursive: true, force: true });
  }
});

test("rejects API errors instead of treating socket data as an acknowledgment", { concurrency: false }, async (t) => {
  if (process.platform === "win32") t.skip("Unix socket smoke test");

  const tempDir = await mkdtemp(path.join(os.tmpdir(), "pi-grok-herdr-error-"));
  const socketPath = path.join(tempDir, "herdr.sock");
  const { requests, server } = await recordingServer(socketPath, (request, socket) => {
    const response = {
      id: request.id,
      error: { code: "pane_not_found", message: "pane not found" },
    };
    const json = JSON.stringify(response) + "\n";
    // A response can arrive in multiple chunks.
    socket.write(json.slice(0, 9));
    socket.end(json.slice(9));
  });
  const saved = {
    env: process.env.HERDR_ENV,
    socket: process.env.HERDR_SOCKET_PATH,
    pane: process.env.HERDR_PANE_ID,
  };
  const logged = [];
  const originalError = console.error;
  console.error = (...args) => logged.push(args.join(" "));
  process.env.HERDR_ENV = "1";
  process.env.HERDR_SOCKET_PATH = socketPath;
  process.env.HERDR_PANE_ID = "w-test:p3";

  try {
    const { default: install } = await importFresh(tempDir, "error");
    const h = harness();
    install(h.pi);
    h.lifecycle.get("session_start")({ reason: "startup" }, {
      hasUI: true,
      isIdle: () => true,
      sessionManager: {
        getSessionFile: () => undefined,
        getSessionId: () => "session-error",
      },
    });

    await waitFor(() => logged.length > 0, "failed API response diagnostic");
    assert.match(logged[0], /pane.report_agent_session failed: pane_not_found/);
    assert.equal(requests[0].id, requests[1].id, "retry preserves request identity");
    assert.equal(requests[0].params.seq, requests[1].params.seq);
    await waitFor(() => requests.some((r) => r.method === "pane.report_agent"), "state after session failure");
  } finally {
    console.error = originalError;
    await new Promise((resolve) => server.close(resolve));
    if (saved.env === undefined) delete process.env.HERDR_ENV;
    else process.env.HERDR_ENV = saved.env;
    if (saved.socket === undefined) delete process.env.HERDR_SOCKET_PATH;
    else process.env.HERDR_SOCKET_PATH = saved.socket;
    if (saved.pane === undefined) delete process.env.HERDR_PANE_ID;
    else process.env.HERDR_PANE_ID = saved.pane;
    await rm(tempDir, { recursive: true, force: true });
  }
});
