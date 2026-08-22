/** Bridge envelopes and persistence records for pi-grok-subagents. */

import { connect, type Socket } from "node:net";
import { SessionManager, type AgentSession, type ExtensionAPI, type ExtensionCommandContext, type ExtensionContext } from "@earendil-works/pi-coding-agent";
import { BRIDGE_TYPE, SHORT_SUBAGENT_ID_LENGTH, STATE_ENTRY_TYPE, textFromContent, type CapabilityMode } from "./shared.ts";

export type BridgeKind = "spawned" | "finished" | "child_update";

export type BridgeEnvelope = {
  version: 1;
  sequence: number;
  replay: boolean;
  kind: BridgeKind;
  parentSessionId: string;
  subagentId: string;
  childSessionId: string;
  payload: Record<string, unknown>;
};

export type PersistedRecord = {
  version: 1;
  id: string;
  childSessionId: string;
  childSessionFile: string;
  parentSessionId: string;
  parentToolCallId: string;
  prompt: string;
  description: string;
  type: string;
  capabilityMode: CapabilityMode;
  modelId: string;
  background: boolean;
  startedAt: number;
  status: "running" | "completed" | "failed" | "cancelled";
  turnCount: number;
  toolCallCount: number;
  tokensUsed: number;
  lastError?: string;
};

export type SubagentRecord = {
  id: string;
  childSessionId: string;
  childSessionFile: string;
  parentSessionId: string;
  parentToolCallId: string;
  prompt: string;
  description: string;
  type: string;
  capabilityMode: CapabilityMode;
  modelId: string;
  background: boolean;
  startedAt: number;
  session: AgentSession;
  turnCount: number;
  toolCallCount: number;
  toolsUsed: Set<string>;
  errorCount: number;
  tokensUsed: number;
  finished: boolean;
  /** Terminal status set by finish(): "completed" | "failed" | "cancelled". */
  terminalStatus: "completed" | "failed" | "cancelled" | null;
  /** Error message from finish(), if the subagent failed. */
  lastError?: string;
  /** Final assistant text captured when this run finished. Required when V2 reuses the child session later. */
  finalOutputText?: string;
  cancelRequested: boolean;
  /** Max turns before injecting a summary prompt. 0 = unlimited. */
  maxTurns: number;
  /** Set when turn limit triggers abort-then-summarize. */
  turnLimitReached: boolean;
  /** Resolved when finish() is called — enables true blocking wait. */
  donePromise: Promise<void>;
  doneResolve: () => void;
  removeAbortListener: () => void;
  unsubscribe: () => void;
};

export interface RecordLookup {
  has(id: string): boolean;
}

export type BridgeRef = Pick<SubagentRecord, "id" | "childSessionId" | "parentSessionId">;

export type BridgeEmitter = (
  record: BridgeRef,
  kind: BridgeKind,
  payload: Record<string, unknown>,
  replay?: boolean,
) => void;

function createTransientTransport(): Socket | undefined {
  const endpoint = process.env.PI_GROK_SUBAGENT_SOCKET?.trim();
  if (!endpoint) return undefined;
  const socket = connect({ path: endpoint });
  socket.unref();
  socket.on("error", () => {});
  return socket;
}

export function createBridgeEmitter(pi: ExtensionAPI): BridgeEmitter {
  let nextSequence = 1;
  const transport = createTransientTransport();
  return (record, kind, payload, replay = false) => {
    const envelope: BridgeEnvelope = {
      version: 1,
      sequence: nextSequence,
      replay,
      kind,
      parentSessionId: record.parentSessionId,
      subagentId: record.id,
      childSessionId: record.childSessionId,
      payload,
    };
    nextSequence += 1;

    // Child traffic is transport state, not parent conversation state. The
    // process-private loopback stream keeps it off disk and out of the parent
    // SessionManager tree while preserving event order and low latency.
    transport?.write(`${JSON.stringify({ type: "custom", customType: BRIDGE_TYPE, data: envelope })}\n`);

    // Parent persistence needs only durable lifecycle anchors. Replay and
    // child_update events must never mutate the parent session JSONL.
    if (!replay && (kind === "spawned" || kind === "finished")) {
      pi.appendEntry(BRIDGE_TYPE, envelope);
    }
  };
}

export function persistedRecord(entry: unknown): PersistedRecord | undefined {
  if (typeof entry !== "object" || entry === null) return undefined;
  const candidate = entry as { type?: unknown; customType?: unknown; data?: unknown };
  if (candidate.type !== "custom" || candidate.customType !== STATE_ENTRY_TYPE) return undefined;
  if (typeof candidate.data !== "object" || candidate.data === null) return undefined;
  const value = candidate.data as Partial<PersistedRecord>;
  if (
    value.version !== 1 ||
    typeof value.id !== "string" ||
    typeof value.childSessionId !== "string" ||
    typeof value.childSessionFile !== "string" ||
    typeof value.parentSessionId !== "string" ||
    typeof value.parentToolCallId !== "string" ||
    typeof value.prompt !== "string" ||
    typeof value.description !== "string" ||
    typeof value.type !== "string" ||
    typeof value.capabilityMode !== "string" ||
    typeof value.modelId !== "string" ||
    typeof value.background !== "boolean" ||
    typeof value.startedAt !== "number" ||
    typeof value.status !== "string" ||
    typeof value.turnCount !== "number" ||
    typeof value.toolCallCount !== "number" ||
    typeof value.tokensUsed !== "number" ||
    (value.lastError !== undefined && typeof value.lastError !== "string")
  ) {
    return undefined;
  }
  return value as PersistedRecord;
}

export function latestPersistedRecords(ctx: ExtensionContext, allBranches = false): PersistedRecord[] {
  const latest = new Map<string, PersistedRecord>();
  const parentSessionId = ctx.sessionManager.getSessionId();
  const entries = allBranches ? ctx.sessionManager.getEntries() : ctx.sessionManager.getBranch();
  for (const entry of entries) {
    const snapshot = persistedRecord(entry);
    if (snapshot?.parentSessionId === parentSessionId) latest.set(snapshot.id, snapshot);
  }
  return [...latest.values()].sort((left, right) => left.startedAt - right.startedAt);
}

export function persistedStatusLabel(
  records: RecordLookup,
  snapshot: PersistedRecord,
): string {
  if (snapshot.status === "running" && !records.has(snapshot.id)) return "INTERRUPTED";
  return snapshot.status.toUpperCase();
}

export function resolvePersistedSubagent(snapshots: PersistedRecord[], id: string): PersistedRecord {
  const exact = snapshots.find((snapshot) => snapshot.id === id);
  if (exact) return exact;
  if (id.length < SHORT_SUBAGENT_ID_LENGTH) {
    throw new Error(`subagent ID prefix is too short: ${id}; use at least ${SHORT_SUBAGENT_ID_LENGTH} characters`);
  }
  const matches = snapshots.filter((snapshot) => snapshot.id.startsWith(id));
  if (matches.length === 1) return matches[0];
  if (matches.length > 1) throw new Error(`ambiguous subagent ID prefix: ${id}; use more characters`);
  throw new Error(`unknown subagent history: ${id}`);
}

/** Shortest unique prefix of `id` among `candidateIds` (at least SHORT_SUBAGENT_ID_LENGTH). */
export function shortSubagentIdFor(id: string, candidateIds: Iterable<string>): string {
  const candidates = [...candidateIds];
  for (let length = SHORT_SUBAGENT_ID_LENGTH; length < id.length; length += 1) {
    const prefix = id.slice(0, length);
    const matches = candidates.filter((candidate) => candidate.startsWith(prefix));
    if (matches.length <= 1) return prefix;
  }
  return id;
}

function historyValue(value: unknown): string {
  const text = textFromContent(value).trim();
  if (text) return text;
  if (value === undefined) return "";
  try {
    return JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    return String(value);
  }
}

function formatPersistedSubagentHistory(
  records: RecordLookup,
  snapshot: PersistedRecord,
  snapshots: PersistedRecord[],
): string {
  const branch = SessionManager.open(snapshot.childSessionFile).getBranch();
  const shortId = shortSubagentIdFor(snapshot.id, snapshots.map((candidate) => candidate.id));
  const lines = [
    `# Subagent ${shortId}: ${snapshot.description}`,
    `Status: ${persistedStatusLabel(records, snapshot)} · ${snapshot.type} · ${snapshot.turnCount} turns · ${snapshot.toolCallCount} tools`,
    `Child session: ${snapshot.childSessionId}`,
    ...(snapshot.lastError ? [`Error: ${snapshot.lastError}`] : []),
  ];

  for (const entry of branch) {
    if (typeof entry !== "object" || entry === null) continue;
    const item = entry as { type?: unknown; summary?: unknown; message?: unknown };
    if (item.type === "compaction" && typeof item.summary === "string" && item.summary.trim()) {
      lines.push(`## Earlier summary\n${item.summary.trim()}`);
      continue;
    }
    if (typeof item.message !== "object" || item.message === null) continue;
    const message = item.message as { role?: unknown; content?: unknown; toolName?: unknown; isError?: unknown };
    if (message.role === "user") {
      const text = historyValue(message.content);
      if (text) lines.push(`## User\n${text}`);
      continue;
    }
    if (message.role === "assistant" && Array.isArray(message.content)) {
      const parts: string[] = [];
      for (const block of message.content) {
        if (typeof block !== "object" || block === null) continue;
        const value = block as { type?: unknown; text?: unknown; thinking?: unknown; name?: unknown; arguments?: unknown };
        if (value.type === "text" && typeof value.text === "string" && value.text.trim()) {
          parts.push(value.text.trim());
        } else if (value.type === "thinking" && typeof value.thinking === "string" && value.thinking.trim()) {
          parts.push(`### Thinking\n${value.thinking.trim()}`);
        } else if (value.type === "toolCall" && typeof value.name === "string") {
          const args = historyValue(value.arguments);
          parts.push(`### Tool call · ${value.name}${args ? `\n${args}` : ""}`);
        }
      }
      if (parts.length > 0) lines.push(`## Assistant\n${parts.join("\n\n")}`);
      continue;
    }
    if (message.role === "toolResult") {
      const text = historyValue(message.content);
      const name = typeof message.toolName === "string" ? ` · ${message.toolName}` : "";
      const error = message.isError === true ? " · ERROR" : "";
      if (text) lines.push(`## Tool result${name}${error}\n${text}`);
    }
  }

  return lines.join("\n\n");
}

export async function showSubagentHistory(
  records: RecordLookup,
  args: string,
  ctx: ExtensionCommandContext,
): Promise<void> {
  const snapshots = latestPersistedRecords(ctx);
  if (snapshots.length === 0) {
    ctx.ui.notify("No subagent history is available for this session.", "warning");
    return;
  }

  let snapshot: PersistedRecord | undefined;
  const supplied = args.trim();
  if (supplied) {
    try {
      snapshot = resolvePersistedSubagent(snapshots, supplied);
    } catch (error) {
      ctx.ui.notify(error instanceof Error ? error.message : String(error), "error");
      return;
    }
  } else {
    const ids = snapshots.map((candidate) => candidate.id);
    const choices = [...snapshots].reverse().map((candidate) => {
      const shortId = shortSubagentIdFor(candidate.id, ids);
      return `${shortId} · [${persistedStatusLabel(records, candidate)}] ${candidate.description}`;
    });
    const selected = await ctx.ui.select("Subagent history", choices);
    if (!selected) return;
    const selectedId = selected.split(" · ", 1)[0];
    snapshot = resolvePersistedSubagent(snapshots, selectedId);
  }

  try {
    const ids = snapshots.map((candidate) => candidate.id);
    const shortId = shortSubagentIdFor(snapshot.id, ids);
    const history = formatPersistedSubagentHistory(records, snapshot, snapshots);
    await ctx.ui.editor(`Subagent ${shortId} history (changes are ignored)`, history);
  } catch (error) {
    ctx.ui.notify(`Unable to open subagent history: ${error instanceof Error ? error.message : String(error)}`, "error");
  }
}

export function persist(pi: ExtensionAPI, record: SubagentRecord, status: PersistedRecord["status"]): void {
  const snapshot: PersistedRecord = {
    version: 1,
    id: record.id,
    childSessionId: record.childSessionId,
    childSessionFile: record.childSessionFile,
    parentSessionId: record.parentSessionId,
    parentToolCallId: record.parentToolCallId,
    prompt: record.prompt,
    description: record.description,
    type: record.type,
    capabilityMode: record.capabilityMode,
    modelId: record.modelId,
    background: record.background,
    startedAt: record.startedAt,
    status,
    turnCount: record.turnCount,
    toolCallCount: record.toolCallCount,
    tokensUsed: record.tokensUsed,
    lastError: record.lastError,
  };
  pi.appendEntry(STATE_ENTRY_TYPE, snapshot);
}
