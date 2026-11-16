import assert from "node:assert/strict";
import { createServer } from "node:net";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { setTimeout as sleep } from "node:timers/promises";

// Only the transport is stubbed. The actual bundled Herdr lifecycle module
// supplies the state machine, ordering, sequence IDs, and socket client.
test("Herdr reports authoritative session, idle, working and blocked states", async () => {
  const dir = await mkdtemp(join(tmpdir(), "pi-herdr-state-"));
  const socketPath = join(dir, "socket");
  const reports = [];
  let holdResponses = false;
  const heldReplies = [];
  const server = createServer((socket) => {
    let buffer = "";
    socket.on("data", (bytes) => {
      buffer += bytes.toString();
      const newline = buffer.indexOf("\n");
      if (newline < 0) return;
      const request = JSON.parse(buffer.slice(0, newline));
      reports.push(request);
      const reply = () =>
        socket.end(JSON.stringify({ id: request.id, result: { ok: true } }) + "\n");
      if (holdResponses) heldReplies.push(reply);
      else reply();
    });
  });

  try {
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(socketPath, resolve);
    });
    process.env.HERDR_ENV = "1";
    process.env.HERDR_SOCKET_PATH = socketPath;
    process.env.HERDR_PANE_ID = "isolated-herdr-pane";

    // Herdr module reads its environment at import time, not on event delivery.
    const { default: installHerdrLifecycle } = await import("./index.ts");
    const lifecycle = new Map();
    const events = new Map();
    const pi = {
      on: (name, handler) => lifecycle.set(name, handler),
      events: { on: (name, handler) => events.set(name, handler) },
    };
    installHerdrLifecycle(pi);

    const ctx = {
      hasUI: true,
      isIdle: () => true,
      sessionManager: {
        getSessionId: () => "isolated-session-id",
        getSessionFile: () => join(dir, "session.jsonl"),
      },
    };
    const waitFor = async (predicate, description) => {
      const deadline = Date.now() + 3000;
      while (Date.now() < deadline) {
        const report = reports.find(predicate);
        if (report) return report;
        await sleep(15);
      }
      throw new Error("Missing Herdr report: " + description);
    };
    const stateReport = (state, afterSeq = 0) =>
      waitFor(
        (r) =>
          r.method === "pane.report_agent" &&
          r.params.state === state &&
          r.params.seq > afterSeq,
        state,
      );

    // Welcome's prewarmed non-interactive session must not steal pane ownership.
    lifecycle.get("session_start")({ reason: "startup" }, { ...ctx, hasUI: false });
    await sleep(40);
    assert.equal(reports.length, 0);

    lifecycle.get("session_start")({ reason: "new" }, ctx);
    await waitFor((r) => r.method === "pane.report_agent_session", "session");
    const initial = await stateReport("idle");

    lifecycle.get("agent_start")({}, ctx);
    const working = await stateReport("working", initial.params.seq);

    events.get("herdr:blocked")({ active: true, label: "approval" });
    const blocked = await stateReport("blocked", working.params.seq);
    assert.equal(blocked.params.message, "approval");

    events.get("herdr:blocked")({ active: false });
    const resumed = await stateReport("working", blocked.params.seq);

    lifecycle.get("agent_settled")({}, ctx);
    const settled = await stateReport("idle", resumed.params.seq);

    assert.ok(initial.params.seq < working.params.seq);
    assert.ok(working.params.seq < blocked.params.seq);
    assert.ok(blocked.params.seq < resumed.params.seq);
    assert.ok(resumed.params.seq < settled.params.seq);

    // A fast turn can finish before socket responses return. Queueing must
    // preserve transitions (not coalesce away blocked/working/idle).
    lifecycle.get("agent_start")({}, ctx);
    events.get("herdr:blocked")({ active: true, label: "rapid approval" });
    events.get("herdr:blocked")({ active: false });
    lifecycle.get("agent_settled")({}, ctx);
    const rapidWorking = await stateReport("working", settled.params.seq);
    const rapidBlocked = await stateReport("blocked", rapidWorking.params.seq);
    const rapidResumed = await stateReport("working", rapidBlocked.params.seq);
    const rapidIdle = await stateReport("idle", rapidResumed.params.seq);
    assert.equal(rapidBlocked.params.message, "rapid approval");
    assert.ok(rapidIdle.params.seq > rapidResumed.params.seq);

    // A dead Herdr socket cannot allow an unbounded queue to retain the
    // entire turn history. Once it recovers, the latest session and state
    // must still reach Herdr even if intermediate transitions were collapsed.
    const reportsBeforeBackpressure = reports.length;
    const cycles = 200;
    holdResponses = true;
    for (let i = 0; i < cycles; i++) {
      lifecycle.get("agent_start")({}, ctx);
      lifecycle.get("agent_settled")({}, ctx);
    }
    await sleep(30);
    holdResponses = false;
    for (const reply of heldReplies.splice(0)) reply();
    const finalSeq = rapidIdle.params.seq + cycles * 3;
    await waitFor(
      (r) => r.method === "pane.report_agent" &&
        r.params.seq === finalSeq && r.params.state === "idle",
      "latest idle after socket stall",
    );
    assert.ok(
      reports.length - reportsBeforeBackpressure <= 140,
      "Socket stall must not retain an unlimited number of queued reports",
    );

    for (const report of reports) {
      assert.equal(report.params.pane_id, "isolated-herdr-pane");
      assert.equal(report.params.source, "herdr:pi");
      assert.equal(report.params.agent, "pi");
      assert.equal(report.params.agent_session_path, join(dir, "session.jsonl"));
    }
    console.log("PASS Herdr authoritative lifecycle: idle → working → blocked → working → idle");
  } finally {
    holdResponses = false;
    for (const reply of heldReplies.splice(0)) reply();
    delete process.env.HERDR_ENV;
    delete process.env.HERDR_SOCKET_PATH;
    delete process.env.HERDR_PANE_ID;
    await new Promise((resolve) => server.close(resolve));
    await rm(dir, { recursive: true, force: true });
  }
});
