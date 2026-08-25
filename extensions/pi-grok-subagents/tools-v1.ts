/** V1 command and tool surface. Runtime behavior stays compatible with existing grok-pi subagents. */

import { Type } from "typebox";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { configureSubagents } from "./config-ui.ts";
import { showSubagentHistory, type SubagentRecord } from "./bridge.ts";
import { MAX_WAIT_MS, SUBAGENT_REPLAY_COMMAND, requireText } from "./shared.ts";
import { SubagentRuntime, type MessageDelivery } from "./runtime.ts";

async function sendMessageFromCommand(
  runtime: SubagentRuntime,
  args: string,
  ctx: ExtensionCommandContext,
): Promise<void> {
  const running = [...runtime.records.values()].filter((record) => !record.finished);
  if (running.length === 0) {
    ctx.ui.notify("No running subagents can receive a message.", "warning");
    return;
  }
  const supplied = args.trim();
  const [candidateId, ...candidateMessage] = supplied.split(/\s+/).filter(Boolean);
  let record: SubagentRecord | undefined;
  let message = supplied;
  if (candidateId) {
    record = runtime.tryResolveSubagent(candidateId);
    if (record) {
      message = candidateMessage.join(" ");
    } else if (/^[0-9a-f-]{8,}$/i.test(candidateId)) {
      try {
        runtime.resolveSubagent(candidateId);
      } catch (error) {
        ctx.ui.notify(error instanceof Error ? error.message : String(error), "error");
        return;
      }
    }
  }
  if (!record) {
    const choices = running.map((item) => ({
      id: item.id,
      label: `${runtime.shortSubagentId(item.id)} · ${item.description}`,
    }));
    const selected = await ctx.ui.select("Send message to subagent", choices.map((choice) => choice.label));
    if (!selected) return;
    const choice = choices.find((candidate) => candidate.label === selected);
    record = choice ? runtime.records.get(choice.id) : undefined;
    if (!record) return;
  }
  if (record.finished) {
    ctx.ui.notify(`Subagent ${runtime.shortSubagentId(record.id)} has already finished.`, "warning");
    return;
  }
  if (!message) {
    const input = await ctx.ui.input("Message for subagent", "What should the subagent do next?");
    if (!input?.trim()) return;
    message = input.trim();
  }
  const deliveryChoice = await ctx.ui.select("Delivery mode", [
    "Follow up (after current turn)",
    "Steer (interrupt current turn)",
  ]);
  if (!deliveryChoice) return;
  const delivery: MessageDelivery = deliveryChoice.startsWith("Steer") ? "steer" : "follow_up";
  runtime.sendMessage(record, message, delivery);
  ctx.ui.notify(`Sent ${delivery === "steer" ? "steer" : "follow-up"} message to ${record.description}.`, "info");
}

export function registerV1Tools(pi: ExtensionAPI, runtime: SubagentRuntime): void {
  pi.registerCommand("subagents", {
    description: "Configure project/global Pi subagents",
    handler: async (_args, ctx) => {
      await configureSubagents(pi, ctx);
    },
  });

  pi.registerCommand("subagent-message", {
    description: "Send a steer or follow-up message to a running Pi subagent",
    handler: async (args, ctx) => {
      await sendMessageFromCommand(runtime, args, ctx);
    },
  });

  pi.registerCommand("subagent-history", {
    description: "View the persisted transcript for a current or finished Pi subagent",
    handler: async (args, ctx) => {
      await showSubagentHistory(runtime.records, args, ctx);
    },
  });

  pi.registerTool({
    name: "spawn_subagent",
    label: "Spawn Subagent",
    description:
      "Launch an autonomous Pi child session shown in Grok's native subagent UI.\n\n" +
      "Usage notes:\n" +
      "- Set background=true to run asynchronously; returns the subagent ID immediately\n" +
      "- For background subagents, use get_command_or_subagent_output with task_ids and timeout_ms to wait for results\n" +
      "- Without background (default), blocks until the subagent finishes and returns its final output directly\n" +
      "- Do NOT use wait_tasks for subagent IDs — use get_command_or_subagent_output instead\n" +
      "- You can spawn multiple background subagents in parallel (up to 4 concurrent)",
    executionMode: "parallel",
    parameters: Type.Object({
      prompt: Type.String({ description: "Self-contained task for the child agent. Include all context needed — the child cannot see your conversation." }),
      description: Type.String({ description: "Short 3-5 word task label shown in the subagent UI." }),
      subagent_type: Type.Optional(Type.String({ description: "Agent profile: general-purpose (default), explore (read-only research), or plan (planning only)." })),
      background: Type.Optional(Type.Boolean({ description: "Run asynchronously and return the child ID immediately. Use get_command_or_subagent_output(task_ids, timeout_ms) to collect results." })),
      model: Type.Optional(Type.String({ description: "Optional Pi model callback. When the selected subagent Markdown definition has models, it must be one of its up-to-three enabled models." })),
      max_turns: Type.Optional(Type.Integer({ minimum: 0, description: "Soft maximum child turns. At the limit Pi receives one end-and-summarize steering message; 0 means unlimited. A Markdown definition takes precedence." })),
      capability_mode: Type.Optional(Type.String({ description: "Tool access: read-only, read-write, execute, or all. Defaults to profile capability." })),
    }),
    async execute(toolCallId, params, signal, _onUpdate, ctx) {
      const record = await runtime.createRecord(toolCallId, params, signal, ctx);
      if (record.background) {
        runtime.scheduleBackground(record, params.prompt);
        const shortId = runtime.shortSubagentId(record.id);
        return {
          content: [{ type: "text", text: `Started background subagent ${shortId}.\nUse get_command_or_subagent_output with task_ids=["${shortId}"] and timeout_ms to wait for its result.\nHistory: /subagent-history ${shortId}` }],
          details: { subagentId: record.id, childSessionId: record.childSessionId, agentSessionId: record.agentSessionId, background: true },
        };
      }
      const output = await runtime.run(record, params.prompt);
      return {
        content: [{ type: "text", text: `${output || "Subagent completed without text output."}\n\nHistory: /subagent-history ${runtime.shortSubagentId(record.id)}` }],
        details: { subagentId: record.id, childSessionId: record.childSessionId, agentSessionId: record.agentSessionId, background: false },
      };
    },
  });

  pi.registerTool({
    name: "send_message_to_subagent",
    label: "Send Message to Subagent",
    description:
      "Send a message to a running Pi subagent. delivery=follow_up queues it after the current turn; " +
      "delivery=steer interrupts the current turn and delivers it immediately.",
    parameters: Type.Object({
      task_id: Type.Optional(Type.String({ description: "Running subagent ID or unique 8+ character prefix." })),
      subagent_id: Type.Optional(Type.String({ description: "Running subagent ID or unique 8+ character prefix (alternative to task_id)." })),
      message: Type.String({ description: "Message or updated instruction for the child session." }),
      delivery: Type.Optional(Type.Union([
        Type.Literal("follow_up"),
        Type.Literal("steer"),
      ], { description: "follow_up queues after the current turn; steer interrupts it. Defaults to follow_up." })),
    }),
    async execute(_toolCallId, params) {
      const id = requireText(params.task_id ?? params.subagent_id, "task_id or subagent_id");
      const message = requireText(params.message, "message");
      const delivery: MessageDelivery = params.delivery === "steer" ? "steer" : "follow_up";
      const record = runtime.runningSubagent(id);
      runtime.sendMessage(record, message, delivery);
      return {
        content: [{ type: "text", text: `Queued ${delivery === "steer" ? "steer" : "follow-up"} message for subagent ${runtime.shortSubagentId(record.id)}.` }],
        details: { subagentId: record.id, delivery, accepted: true },
      };
    },
  });

  pi.registerTool({
    name: "get_command_or_subagent_output",
    label: "Get Subagent Output",
    description:
      "Get output and status from one or more background subagents.\n\n" +
      "Usage notes:\n" +
      "- Pass task_ids with one or more subagent IDs from background=true spawn_subagent calls\n" +
      "- For a single subagent use a one-element array: task_ids=[\"<id>\"]\n" +
      "- Set a positive timeout_ms to block until all listed subagents complete (or timeout). Recommended: 120000–600000\n" +
      "- Omit timeout_ms or pass 0 for a non-blocking status snapshot\n" +
      "- Returns status, progress, and final output text for each subagent\n" +
      "- Do NOT use wait_tasks for subagent IDs — this tool handles waiting",
    parameters: Type.Object({
      task_ids: Type.Optional(Type.Array(Type.String(), { description: "One or more subagent IDs or unique 8+ character prefixes to check." })),
      subagent_id: Type.Optional(Type.String({ description: "Single subagent ID or unique 8+ character prefix (alternative to task_ids for one subagent)." })),
      timeout_ms: Type.Optional(Type.Number({ description: "Max milliseconds to wait for completion. 0 or omitted = non-blocking snapshot. Capped at 600000 (10 min)." })),
    }),
    async execute(_toolCallId, params, signal) {
      const requestedIds: string[] = params.task_ids?.length
        ? params.task_ids
        : params.subagent_id
          ? [params.subagent_id]
          : [];
      if (requestedIds.length === 0) throw new Error("Provide task_ids (array) or subagent_id (string) with at least one subagent ID");
      const resolvedRecords = requestedIds.map((id) => runtime.resolveSubagent(id));
      const ids = resolvedRecords.map((record) => record.id);
      const timeoutMs = Math.min(Math.max(params.timeout_ms ?? 0, 0), MAX_WAIT_MS);
      if (timeoutMs > 0) await runtime.waitForRecords(ids, timeoutMs, signal);
      const results = resolvedRecords.map((record) => runtime.formatSubagentResult(record));
      const allFinished = resolvedRecords.every((record) => record.finished);
      const summary = allFinished
        ? "All subagents finished."
        : "Some subagents still running. Call again with a larger timeout_ms to wait longer.";
      return {
        content: [{ type: "text", text: `${summary}\n\n${results.join("\n\n---\n\n")}` }],
        details: {
          subagents: resolvedRecords.map((record) => ({
            subagentId: record.id,
            finished: record.finished,
            status: runtime.statusLabel(record),
            turns: record.turnCount,
            toolCalls: record.toolCallCount,
          })),
        },
      };
    },
  });

  pi.registerTool({
    name: "kill_command_or_subagent",
    label: "Cancel Subagent",
    description: "Cancel a running background subagent by ID. The subagent will be aborted and marked as cancelled.",
    parameters: Type.Object({
      task_id: Type.Optional(Type.String({ description: "The subagent ID or unique 8+ character prefix to cancel." })),
      subagent_id: Type.Optional(Type.String({ description: "The subagent ID or unique 8+ character prefix to cancel (alternative to task_id)." })),
    }),
    async execute(_toolCallId, params) {
      const id = requireText(params.task_id ?? params.subagent_id, "task_id or subagent_id");
      const record = runtime.resolveSubagent(id);
      if (record.finished) {
        return {
          content: [{ type: "text", text: `Subagent ${runtime.shortSubagentId(record.id)} already finished (${runtime.statusLabel(record)}).` }],
          details: { subagentId: record.id, finished: true },
        };
      }
      runtime.cancel(record);
      return {
        content: [{ type: "text", text: `Cancelled subagent ${runtime.shortSubagentId(record.id)} (${record.description}).` }],
        details: { subagentId: record.id, finished: false },
      };
    },
  });

  pi.registerTool({
    name: "list_subagents",
    label: "List Subagents",
    description: "List all subagents in this session with their current status, progress, and IDs.",
    parameters: Type.Object({}),
    async execute() {
      if (runtime.records.size === 0) {
        return { content: [{ type: "text", text: "No subagents have been spawned in this session." }], details: { subagents: [] } };
      }
      const lines = [...runtime.records.values()]
        .sort((a, b) => a.startedAt - b.startedAt)
        .map((record) => {
          const elapsed = runtime.formatDuration(Date.now() - record.startedAt);
          const bg = record.background ? "bg" : "fg";
          const shortId = runtime.shortSubagentId(record.id);
          return `• [${runtime.statusLabel(record)}] ${shortId} "${record.description}" (${bg}, ${record.type}) — ${elapsed}, ${record.turnCount} turns, ${record.toolCallCount} tools — /subagent-history ${shortId}`;
        });
      return {
        content: [{ type: "text", text: `Subagents (${runtime.records.size}):\n${lines.join("\n")}` }],
        details: {
          subagents: [...runtime.records.values()].map((record) => ({
            subagentId: record.id,
            finished: record.finished,
            status: runtime.statusLabel(record),
          })),
        },
      };
    },
  });

  pi.registerCommand(SUBAGENT_REPLAY_COMMAND, {
    description: "Internal Pi-Grok bridge command: replay persisted subagents after session/load",
    handler: async (args, ctx) => {
      const request = JSON.parse(args || "{}") as { mode?: unknown; requestId?: unknown };
      const mode = request.mode === "recovery" ? "recovery" : "load";
      const requestId = requireText(request.requestId, "replay request id");
      await runtime.replayPersisted(ctx, mode, requestId);
    },
  });

  pi.registerCommand("__pi_grok_subagent_cancel", {
    description: "Internal Pi-Grok bridge command: cancel a subagent",
    handler: async (args) => {
      const id = requireText(args, "subagent id");
      runtime.cancel(runtime.resolveSubagent(id));
    },
  });
}
