// Built into grok-pi. Reports Pi lifecycle state to the Herdr local socket.
import net from "node:net";

const HERDR_ENV = process.env.HERDR_ENV;
const HERDR_SOCKET_PATH = process.env.HERDR_SOCKET_PATH;
const HERDR_PANE_ID = process.env.HERDR_PANE_ID;
const SOURCE = "herdr:pi";
const AGENT = "pi";
const SOCKET_ENDPOINT =
  process.platform === "win32" && HERDR_SOCKET_PATH
    ? `\\\\.\\pipe\\${HERDR_SOCKET_PATH}`
    : HERDR_SOCKET_PATH;

function isEnabled() {
  return HERDR_ENV === "1" && Boolean(SOCKET_ENDPOINT) && Boolean(HERDR_PANE_ID);
}

function requestId(kind) {
  return `${SOURCE}:${kind}:${Date.now()}:${Math.random().toString(36).slice(2)}`;
}

function deliverOnce(request, timeoutMs) {
  return new Promise((resolve) => {
    const socket = net.createConnection(SOCKET_ENDPOINT);
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
      if (buffer.length > 65536) {
        finish({ ok: false, reason: "response too large" });
        return;
      }
      const newline = buffer.indexOf("\n");
      if (newline < 0) return;
      try {
        const response = JSON.parse(buffer.slice(0, newline));
        if (response?.id === request.id && response.result && !response.error) {
          finish({ ok: true });
        } else {
          finish({ ok: false, reason: response?.error?.code ?? "invalid response" });
        }
      } catch {
        finish({ ok: false, reason: "invalid JSON" });
      }
    });
    socket.on("end", () => finish({ ok: false, reason: "connection closed" }));
    timer = setTimeout(() => finish({ ok: false, reason: "timeout" }), timeoutMs);
    timer.unref?.();
  });
}

let deliveryFailed = false;
async function deliver(request) {
  let result = await deliverOnce(request, 500);
  if (!result.ok) result = await deliverOnce(request, 1500);
  if (result.ok) {
    deliveryFailed = false;
  } else if (!deliveryFailed) {
    // The Pi RPC stderr reader captures this without corrupting JSONL stdout.
    console.error(`[pi-grok-herdr] ${request.method} failed: ${result.reason}`);
    deliveryFailed = true;
  }
}

let sequence = Date.now() * 1000;
let sessionPath;
let sessionId;

function nextSequence() {
  sequence += 1;
  return sequence;
}

function refreshSession(ctx) {
  try {
    const candidate = ctx?.sessionManager?.getSessionFile?.();
    sessionPath =
      typeof candidate === "string" && candidate.startsWith("/")
        ? candidate
        : undefined;
  } catch {
    sessionPath = undefined;
  }

  try {
    const candidate = ctx?.sessionManager?.getSessionId?.();
    sessionId =
      typeof candidate === "string" && candidate.length > 0 ? candidate : undefined;
  } catch {
    sessionId = undefined;
  }
}

function sessionFields() {
  if (sessionPath) return { agent_session_path: sessionPath };
  if (sessionId) return { agent_session_id: sessionId };
  return {};
}

function reportSession(reason) {
  const fields = sessionFields();
  if (Object.keys(fields).length === 0) return;

  enqueue({
    id: requestId("session"),
    method: "pane.report_agent_session",
    params: {
      pane_id: HERDR_PANE_ID,
      source: SOURCE,
      agent: AGENT,
      seq: nextSequence(),
      session_start_source: reason,
      ...fields,
    },
  });
}

function reportState(state, message) {
  enqueue({
    id: requestId("state"),
    method: "pane.report_agent",
    params: {
      pane_id: HERDR_PANE_ID,
      source: SOURCE,
      agent: AGENT,
      state,
      message,
      seq: nextSequence(),
      ...sessionFields(),
    },
  });
}

const pendingReports = [];
const MAX_PENDING_REPORTS = 128;
let sending = false;

function enqueue(request) {
  // These are transitions, not snapshots: coalescing queued states can erase
  // working/blocked when a fast Pi turn completes before the socket responds.
  // publish() already suppresses unchanged states. Preserve FIFO normally.
  pendingReports.push(request);
  if (pendingReports.length > MAX_PENDING_REPORTS) {
    // A disconnected Herdr must not consume unbounded memory. Only under
    // exceptional backpressure retain the newest session identity and the
    // latest state *after* that session, so a recovered socket converges.
    const session = pendingReports.findLast(
      (item) => item.method === "pane.report_agent_session",
    );
    const state = pendingReports.findLast(
      (item) => item.method === "pane.report_agent",
    );
    pendingReports.length = 0;
    if (session) pendingReports.push(session);
    if (state && (!session || state.params.seq > session.params.seq)) {
      pendingReports.push(state);
    }
  }
  if (!sending) void flushReports();
}

async function flushReports() {
  if (sending) return;
  sending = true;
  try {
    while (pendingReports.length > 0) {
      await deliver(pendingReports.shift());
    }
  } finally {
    sending = false;
    if (pendingReports.length > 0) void flushReports();
  }
}

export default function installHerdrLifecycle(pi) {
  if (!isEnabled()) return;

  let rootInteractiveSession = false;
  let agentWorking = false;
  let blockerDepth = 0;
  let blockerLabel;
  let lastState;
  let lastMessage;

  function desiredState() {
    if (blockerDepth > 0) return { state: "blocked", message: blockerLabel };
    return { state: agentWorking ? "working" : "idle", message: undefined };
  }

  function publish(force = false) {
    const next = desiredState();
    if (!force && next.state === lastState && next.message === lastMessage) return;
    lastState = next.state;
    lastMessage = next.message;
    reportState(next.state, next.message);
  }

  pi.events.on("herdr:blocked", (event) => {
    if (!rootInteractiveSession) return;

    if (event?.active) {
      blockerDepth += 1;
      blockerLabel = event.label;
    } else {
      blockerDepth = Math.max(0, blockerDepth - 1);
      if (blockerDepth === 0) blockerLabel = undefined;
    }
    publish();
  });

  pi.on("session_start", (event, ctx) => {
    if (ctx?.hasUI !== true) return;

    rootInteractiveSession = true;
    refreshSession(ctx);
    reportSession(event?.reason);
    blockerDepth = 0;
    blockerLabel = undefined;
    agentWorking = ctx?.isIdle?.() === false;
    publish(true);
  });

  pi.on("agent_start", (_event, ctx) => {
    if (!rootInteractiveSession) return;

    refreshSession(ctx);
    reportSession();
    agentWorking = true;
    publish();
  });

  pi.on("agent_settled", (_event, ctx) => {
    if (!rootInteractiveSession || ctx?.isIdle?.() !== true) return;

    agentWorking = false;
    publish();
  });
}
