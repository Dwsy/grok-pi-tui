// Minimal Herdr lifecycle reporter — mimics the built-in pi-grok-herdr bridge.
// Only reports session identity + state over the local Herdr socket. No Pi, no UI.
//
// Usage:
//   HERDR_ENV=1 HERDR_SOCKET_PATH=/path/herdr.sock HERDR_PANE_ID=wXX:pY \
//     node report.mjs <path|id> <sessionValue> <state> [message]
//
// Examples:
//   node report.mjs path /abs/path/to/session.jsonl idle
//   node report.mjs id   01a1deba-...                 working "thinking"
//
// Receives the same newline-delimited JSON ack as the real bridge and exits non-zero
// if any report is not acknowledged, so it doubles as a probe of Herdr's gate.

import net from "node:net";

const SOURCE = "herdr:pi";
const AGENT = "pi";
const HERDR_ENV = process.env.HERDR_ENV;
const SOCKET = process.env.HERDR_SOCKET_PATH;
const PANE = process.env.HERDR_PANE_ID;

const [, , kind, value, state = "idle", message] = process.argv;

if (HERDR_ENV !== "1" || !SOCKET || !PANE) {
  console.error("Need HERDR_ENV=1, HERDR_SOCKET_PATH, HERDR_PANE_ID");
  process.exit(2);
}
if (!["path", "id"].includes(kind) || !value) {
  console.error("Usage: node report.mjs <path|id> <sessionValue> <state> [message]");
  process.exit(2);
}

let sequence = Date.now() * 1000;
const nextSeq = () => ++sequence;
const requestId = (k) => `${SOURCE}:${k}:${Date.now()}:${Math.random().toString(36).slice(2)}`;

function deliverOnce(request, timeoutMs) {
  return new Promise((resolve) => {
    const socket = net.createConnection(SOCKET);
    let settled = false;
    let timer;
    let buffer = "";
    const finish = (result) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      socket.destroy();
      resolve(result);
    };
    socket.on("error", () => finish({ ok: false, reason: "socket error" }));
    socket.on("connect", () => socket.write(`${JSON.stringify(request)}\n`));
    socket.on("data", (chunk) => {
      buffer += chunk.toString();
      const newline = buffer.indexOf("\n");
      if (newline < 0) return;
      try {
        const res = JSON.parse(buffer.slice(0, newline));
        finish(res?.id === request.id && res.result && !res.error
          ? { ok: true }
          : { ok: false, reason: res?.error?.code ?? "invalid response" });
      } catch {
        finish({ ok: false, reason: "invalid JSON" });
      }
    });
    socket.on("end", () => finish({ ok: false, reason: "connection closed" }));
    timer = setTimeout(() => finish({ ok: false, reason: "timeout" }), timeoutMs);
    timer.unref?.();
  });
}

async function deliver(request) {
  let r = await deliverOnce(request, 500);
  if (!r.ok) r = await deliverOnce(request, 1500);
  console.log(`[${request.method}] ${r.ok ? "ack" : `FAILED: ${r.reason}`} seq=${request.params.seq}`);
  return r.ok;
}

const sessionField = kind === "path" ? "agent_session_path" : "agent_session_id";

const okSession = await deliver({
  id: requestId("session"),
  method: "pane.report_agent_session",
  params: {
    pane_id: PANE,
    source: SOURCE,
    agent: AGENT,
    seq: nextSeq(),
    session_start_source: "demo",
    [sessionField]: value,
  },
});
const okState = await deliver({
  id: requestId("state"),
  method: "pane.report_agent",
  params: {
    pane_id: PANE,
    source: SOURCE,
    agent: AGENT,
    state,
    message,
    seq: nextSeq(),
    [sessionField]: value,
  },
});

console.log(`reported kind=${kind} state=${state} session=${value}`);
process.exit(okSession && okState ? 0 : 1);
