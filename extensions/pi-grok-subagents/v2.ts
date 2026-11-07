/** Optional Subagents V2 team coordinator and peer-communication tool surface. */

import { Type } from "typebox";
import {
  defineTool,
  type ExtensionAPI,
  type ExtensionContext,
  type ToolDefinition,
} from "@earendil-works/pi-coding-agent";
import type { SubagentRecord } from "./bridge.ts";
import { requireText } from "./shared.ts";
import { SubagentRuntime } from "./runtime.ts";
import {
  renderTeamTemplate,
  selectedTeam,
  type TeamDefinition,
  type TeamMemberDefinition,
} from "./teams.ts";
import { configureTeamPresets } from "./team-ui.ts";

const ROOT_PATH = "/root";
const TEAM_MESSAGE_TYPE = "pi-grok-team-message/v2";
const MAX_TEAM_WAIT_MS = 600_000;
const DEFAULT_TEAM_WAIT_MS = 120_000;

const CONTROL_TOOL_NAMES = [
  "spawn_team_agent",
  "team_send_message",
  "team_followup_task",
  "team_wait",
  "team_list",
  "team_interrupt",
  "team_report",
] as const;

type TeamMessageKind = "MESSAGE" | "NEW_TASK" | "FINAL_ANSWER";
type TeamAgentStatus = "queued" | "running" | "idle" | "waiting_result" | "waiting_decision" | "failed" | "cancelled";

/**
 * Semantic result a child declares with `team_report`. Distinct from the runtime
 * terminal status: `completed` says the turn ended, `ready` says the result is
 * usable, `waiting` leaves a task pending after its run ends, and `dropped`
 * says the parent has nothing to act on.
 */
type TeamReportState = "ready" | "waiting" | "blocked" | "failed" | "dropped";

type TeamReport = {
  state: TeamReportState;
  summary: string;
  question?: string;
};

type TeamAgent = {
  path: string;
  parentPath: string;
  taskName: string;
  role: string;
  description: string;
  record: SubagentRecord;
  status: TeamAgentStatus;
  team?: string;
  report?: TeamReport;
  /** Last time this agent sent or received team traffic; `team_list` reports the quiet gap. */
  lastActivityAt: number;
  pendingTasks: Array<{ senderPath: string; message: string; toolCallId: string; kind: "NEW_TASK" | "FINAL_ANSWER" }>;
};

/**
 * Consecutive `team_send_message` calls from one sender to one target merge into
 * a single steer, so a chatty agent costs the recipient one context entry
 * instead of one per message.
 */
type MessageBuffer = {
  senderPath: string;
  targetPath: string;
  parts: string[];
};

type SpawnAgentArgs = {
  task_name: string;
  message: string;
  agent_type?: string;
  description?: string;
  model?: string;
  max_turns?: number;
  thinking_level?: string;
  capability_mode?: string;
};

function pathSegment(value: string): string {
  const segment = value.trim();
  if (!segment || segment === "root" || !/^[a-z0-9][a-z0-9_]{0,63}$/.test(segment)) {
    throw new Error("task_name must use lowercase letters, digits, and underscores; `root` is reserved");
  }
  return segment;
}

function validateAgentPath(path: string): string {
  if (path === ROOT_PATH) return path;
  if (!path.startsWith(`${ROOT_PATH}/`) || path.endsWith("/")) throw new Error(`invalid agent path: ${path}`);
  for (const segment of path.slice(ROOT_PATH.length + 1).split("/")) pathSegment(segment);
  return path;
}

function childPath(parentPath: string, taskName: string): string {
  return validateAgentPath(`${validateAgentPath(parentPath)}/${pathSegment(taskName)}`);
}

/**
 * Restricts a preset to the requested member subset while preserving preset
 * order: a synthesizer defined last must still start after its producers.
 */
function selectTeamMembers(
  team: TeamDefinition,
  requested: string[] | undefined,
): TeamMemberDefinition[] {
  if (!requested || requested.length === 0) return team.members;
  const known = new Set(team.members.map((member) => member.name));
  const unknown = requested.filter((name) => !known.has(name));
  if (unknown.length > 0) {
    throw new Error(
      `team ${team.name} has no member named ${unknown.join(", ")}; available: ${team.members.map((member) => member.name).join(", ")}`,
    );
  }
  const selected = new Set(requested);
  return team.members.filter((member) => selected.has(member.name));
}

function renderMessage(kind: TeamMessageKind, target: string, sender: string, payload: string): string {
  return `Message Type: ${kind}\nTask name: ${target}\nSender: ${sender}\nPayload:\n${payload}`;
}

function toolText(text: string, details?: Record<string, unknown>) {
  return { content: [{ type: "text" as const, text }], details };
}

export class TeamCoordinator {
  private readonly agents = new Map<string, TeamAgent>();
  private readonly reservedPaths = new Set<string>();
  private readonly waiters = new Set<(version: number) => void>();
  private readonly messageBuffers = new Map<string, MessageBuffer>();
  private readonly pi: ExtensionAPI;
  private readonly runtime: SubagentRuntime;
  private activityVersion = 0;

  constructor(pi: ExtensionAPI, runtime: SubagentRuntime) {
    this.pi = pi;
    this.runtime = runtime;
  }

  register(): void {
    for (const tool of this.controlTools(ROOT_PATH)) this.pi.registerTool(tool);
    this.pi.registerTool(this.spawnTeamTool());
    this.pi.registerCommand("subagent-teams", {
      description: "Configure Subagents V2 team presets, per-member models and thinking levels",
      handler: async (_args, ctx) => {
        await configureTeamPresets(ctx);
      },
    });
  }

  private notifyActivity(): void {
    this.activityVersion += 1;
    for (const resolve of this.waiters) resolve(this.activityVersion);
    this.waiters.clear();
  }

  private touch(agent: TeamAgent): void {
    agent.lastActivityAt = Date.now();
  }

  /** Finished runs keep a live child session while idle or awaiting input. */
  private canReceiveMessage(status: TeamAgentStatus): boolean {
    return status === "idle" || status === "waiting_result" || status === "waiting_decision";
  }

  private bufferMessage(senderPath: string, targetPath: string, message: string): number {
    const key = `${senderPath}\u0000${targetPath}`;
    const existing = this.messageBuffers.get(key);
    if (existing) {
      // Repeating the same text inside one turn adds context without adding meaning.
      if (!existing.parts.includes(message)) existing.parts.push(message);
      return existing.parts.length;
    }
    this.messageBuffers.set(key, { senderPath, targetPath, parts: [message] });
    return 1;
  }

  /** Delivers every buffered sender/target pair as one merged steer. */
  async flushBufferedMessages(): Promise<void> {
    const buffers = [...this.messageBuffers.values()];
    this.messageBuffers.clear();
    for (const buffer of buffers) {
      const payload = buffer.parts.length === 1
        ? buffer.parts[0]
        : buffer.parts.map((part, index) => `${index + 1}. ${part}`).join("\n");
      await this.deliver(buffer.senderPath, buffer.targetPath, "MESSAGE", payload, false);
    }
  }

  private resolveTarget(senderPath: string, target: string): string {
    const trimmed = requireText(target, "target");
    const resolved = trimmed.startsWith("/") ? validateAgentPath(trimmed) : childPath(senderPath, trimmed);
    if (resolved !== ROOT_PATH && !this.agents.has(resolved)) throw new Error(`unknown team agent: ${resolved}`);
    return resolved;
  }

  private agentForPath(path: string): TeamAgent {
    const agent = this.agents.get(path);
    if (!agent) throw new Error(`unknown team agent: ${path}`);
    return agent;
  }

  private status(agent: TeamAgent): TeamAgentStatus {
    if (!agent.record.finished) {
      return this.runtime.backgroundState(agent.record) === "queued" ? "queued" : "running";
    }
    if (agent.record.terminalStatus === "completed") {
      // A completed run may leave a task waiting for a result or a parent
      // decision. Keep that visible while allowing the same session to resume.
      if (agent.report?.state === "blocked") return "waiting_decision";
      if (agent.report?.state === "waiting") return "waiting_result";
      return "idle";
    }
    return agent.record.terminalStatus ?? agent.status;
  }

  private usageHint(path: string, parentPath: string, extra?: string): string {
    return [
      `You are ${path}, an agent in a grok-pi Subagents V2 team. Your parent is ${parentPath}.`,
      "All team agents share the same working directory and can see each other's filesystem changes. Keep writes inside the boundary your task names.",
      "Create a nested child with spawn_team_agent only when your own task explicitly requires further decomposition. Having the tool is not permission to fan out; your parent remains the orchestrator.",
      "Use team_send_message for facts a running peer needs now, team_followup_task to give an agent new work, team_wait only while another agent is genuinely active, team_list to inspect the tree, and team_interrupt to stop an agent.",
      "A message is worth sending only when the recipient cannot get the fact themselves or must act on it. Do not forward progress. Consecutive team_send_message calls to one agent merge into a single steer, so batch facts in one turn.",
      "Before you end a turn, call team_report: ready (the requested result is deliverable), waiting (your own children are pending and no local work remains), blocked (you need one decision; question is mandatory), failed (the work cannot be completed), dropped (there is genuinely nothing for the parent to act on: already fixed, superseded, or out of scope). Your summary replaces your final text in what the parent receives. Never report ready to hide unfinished work.",
      "When your own child results are still pending and no independent work remains, report waiting and END THIS TURN to release the shared execution slot. Reporting alone does not release it. A nested FINAL_ANSWER resumes this same child session; do not park on team_wait for your queued descendants.",
      "When no other agent is active, finish your turn instead of waiting; you become IDLE and team_followup_task can reactivate this same child session later.",
      "Use absolute /root/... paths when messaging siblings.",
      "Report evidence with concrete file paths and command output; do not assert behaviour you did not verify.",
      extra?.trim() ?? "",
    ].filter(Boolean).join("\n\n");
  }

  private async deliver(
    senderPath: string,
    targetPath: string,
    kind: TeamMessageKind,
    payload: string,
    triggerTurn: boolean,
  ): Promise<void> {
    const content = renderMessage(kind, targetPath, senderPath, payload);
    const details = { version: 2, kind, sender: senderPath, target: targetPath };
    if (targetPath === ROOT_PATH) {
      this.pi.sendMessage(
        { customType: TEAM_MESSAGE_TYPE, content, display: true, details },
        { triggerTurn, deliverAs: triggerTurn ? "followUp" : "steer" },
      );
    } else {
      const target = this.agentForPath(targetPath);
      const targetStatus = this.status(target);
      if (target.record.finished && !this.canReceiveMessage(targetStatus)) {
        throw new Error(`team agent ${targetPath} cannot receive messages after ${targetStatus}`);
      }
      if (target.record.finished && triggerTurn) {
        throw new Error(`team agent ${targetPath} is ${targetStatus}; use team_followup_task to reactivate it`);
      }
      await target.record.session.sendCustomMessage(
        { customType: TEAM_MESSAGE_TYPE, content, display: true, details },
        { triggerTurn, deliverAs: triggerTurn ? "followUp" : "steer" },
      );
      this.touch(target);
    }
    this.notifyActivity();
  }

  private async reactivateAgent(
    target: TeamAgent,
    senderPath: string,
    kind: "NEW_TASK" | "FINAL_ANSWER",
    payload: string,
    toolCallId: string,
    signal?: AbortSignal,
  ): Promise<"running" | "queued" | "skipped"> {
    if (!this.canReceiveMessage(this.status(target))) {
      throw new Error(`team agent ${target.path} cannot be reactivated from ${this.status(target)}`);
    }
    // A reactivated run must not inherit the previous run's report: a stale
    // `dropped` would swallow this run's result, and a stale `blocked` would
    // deliver the old question as if it were new.
    target.report = undefined;
    const content = renderMessage(kind, target.path, senderPath, payload);
    const details = { version: 2, kind, sender: senderPath, target: target.path };
    target.record = this.runtime.resumeRecord(
      target.record,
      toolCallId,
      content,
      signal,
      (finishedRecord, status, error) => this.handleFinished(target.path, finishedRecord, status, error),
    );
    target.status = "running";
    const scheduled = this.runtime.scheduleBackgroundTask(target.record, async () => {
      await this.runtime.runCustomMessage(
        target.record,
        { customType: TEAM_MESSAGE_TYPE, content, display: true, details },
        { triggerTurn: true, deliverAs: "followUp" },
      );
    });
    target.status = scheduled === "queued" ? "queued" : "running";
    this.touch(target);
    this.notifyActivity();
    return scheduled;
  }

  private async handleFinished(
    path: string,
    record: SubagentRecord,
    status: "completed" | "failed" | "cancelled",
    error?: string,
  ): Promise<void> {
    const agent = this.agents.get(path);
    if (!agent) return;
    agent.status = status === "completed" ? "idle" : status;
    this.touch(agent);
    this.notifyActivity();
    // A turn that ends must not leave buffered messages behind: flush them before
    // the parent is told the result, so ordering matches what the child said.
    try {
      await this.flushBufferedMessages();
    } catch (flushError) {
      const detail = flushError instanceof Error ? flushError.message : String(flushError);
      this.runtime.recordPostFinishError(record, `Buffered message delivery failed: ${detail}`);
    }
    // Waiting coordinators release their run without reporting partial work as
    // a final result. Descendant results reactivate the same child session.
    // A stale quiet report must never hide a failed/cancelled run.
    const parent = this.agents.get(agent.parentPath);
    // Even a no-op is a dependency completion for a waiting coordinator,
    // including the interval between reporting waiting and ending its run.
    const parentNeedsCompletion = parent?.report?.state === "waiting";
    const quietReport = agent.report?.state === "waiting"
      || (agent.report?.state === "dropped" && !parentNeedsCompletion);
    if (status !== "completed" || !quietReport) {
      await this.deliverFinalAnswer(agent, path, record, status, error);
    }
    if (status === "completed" && agent.pendingTasks.length > 0) {
      const next = agent.pendingTasks.shift();
      if (next) {
        try {
          await this.reactivateAgent(agent, next.senderPath, next.kind, next.message, next.toolCallId);
        } catch (followupError) {
          const detail = followupError instanceof Error ? followupError.message : String(followupError);
          this.runtime.recordPostFinishError(record, `Queued follow-up failed: ${detail}`);
        }
      }
    } else if (status !== "completed") {
      agent.pendingTasks.length = 0;
    }
  }

  /**
   * A declared report replaces the child's free-form final text, so the parent
   * receives one actionable sentence plus an optional decision request instead
   * of a transcript tail. Without a report the previous behaviour is kept.
   */
  private completedPayload(report: TeamReport | undefined, output: string): string {
    if (!report) return output || "Agent completed without text output.";
    const question = report.question ? `\n\nNeeds your decision: ${report.question}` : "";
    return `Report: ${report.state}\n${report.summary}${question}`;
  }

  private async deliverFinalAnswer(
    agent: TeamAgent,
    path: string,
    record: SubagentRecord,
    status: "completed" | "failed" | "cancelled",
    error?: string,
  ): Promise<void> {
    const output = this.runtime.finalOutput(record);
    const payload = status === "completed"
      ? this.completedPayload(agent.report, output)
      : `Agent ${status}.${error ? ` Error: ${error}` : ""}${output ? `\n\nLast output:\n${output}` : ""}`;
    try {
      const parent = agent.parentPath === ROOT_PATH ? undefined : this.agents.get(agent.parentPath);
      const parentStatus = parent ? this.status(parent) : undefined;
      if (parent && parentStatus && this.canReceiveMessage(parentStatus)) {
        await this.reactivateAgent(parent, path, "FINAL_ANSWER", payload, `team-final:${record.id}`);
      } else if (parent && parentStatus === "queued") {
        parent.pendingTasks.push({
          senderPath: path,
          message: payload,
          toolCallId: `team-final:${record.id}`,
          kind: "FINAL_ANSWER",
        });
        this.notifyActivity();
      } else {
        await this.deliver(path, agent.parentPath, "FINAL_ANSWER", payload, true);
      }
    } catch (deliveryError) {
      const detail = deliveryError instanceof Error ? deliveryError.message : String(deliveryError);
      this.runtime.recordPostFinishError(record, `Final-answer delivery failed: ${detail}`);
    }
  }

  private async spawnAgent(
    senderPath: string,
    args: SpawnAgentArgs,
    toolCallId: string,
    signal: AbortSignal | undefined,
    ctx: ExtensionContext,
    options: { team?: string; systemPromptExtra?: string; description?: string; deferStart?: boolean; parentToolCallId?: string } = {},
  ): Promise<TeamAgent> {
    validateAgentPath(senderPath);
    if (senderPath !== ROOT_PATH && !this.agents.has(senderPath)) throw new Error(`unknown spawning agent: ${senderPath}`);
    const taskName = pathSegment(requireText(args.task_name, "task_name"));
    const message = requireText(args.message, "message");
    const path = childPath(senderPath, taskName);
    if (this.agents.has(path) || this.reservedPaths.has(path)) throw new Error(`team agent path already exists: ${path}`);
    this.reservedPaths.add(path);
    try {
      const parentRecord = senderPath === ROOT_PATH ? undefined : this.agents.get(senderPath)?.record;
      const customTools = this.controlTools(path);
      const record = await this.runtime.createRecord(
        toolCallId,
        {
          prompt: message,
          description: options.description ?? args.description ?? taskName,
          subagent_type: args.agent_type ?? "general-purpose",
          background: true,
          model: args.model,
          max_turns: args.max_turns,
          thinking_level: args.thinking_level,
          capability_mode: args.capability_mode,
        },
        signal,
        ctx,
        {
          customTools,
          parentToolCallId: options.parentToolCallId,
          systemPromptSuffix: this.usageHint(path, senderPath, options.systemPromptExtra),
          teamIdentity: {
            agentPath: path,
            parentAgentPath: senderPath,
            team: options.team,
            bridgeParentSessionId: parentRecord?.childSessionId,
          },
          onFinished: (finishedRecord, status, error) => this.handleFinished(path, finishedRecord, status, error),
        },
      );
      const agent: TeamAgent = {
        path,
        parentPath: senderPath,
        taskName,
        role: args.agent_type ?? "general-purpose",
        description: options.description ?? args.description ?? taskName,
        record,
        status: "running",
        team: options.team,
        lastActivityAt: Date.now(),
        pendingTasks: [],
      };
      this.agents.set(path, agent);
      if (!options.deferStart) this.runtime.scheduleBackground(record, message);
      this.notifyActivity();
      return agent;
    } finally {
      this.reservedPaths.delete(path);
    }
  }

  private rollbackTeam(spawned: TeamAgent[], reason: string): void {
    for (const agent of spawned) {
      this.agents.delete(agent.path);
      this.runtime.discard(agent.record, reason);
    }
    this.notifyActivity();
  }

  private hasPendingChild(senderPath: string): boolean {
    return [...this.agents.values()].some((agent) => {
      if (agent.parentPath !== senderPath) return false;
      const status = this.status(agent);
      return status === "running" || status === "queued" || status === "waiting_result";
    });
  }

  private hasActivePeer(senderPath: string): boolean {
    return [...this.agents.values()].some((agent) => {
      if (agent.path === senderPath) return false;
      const status = this.status(agent);
      return status === "running" || status === "queued";
    });
  }

  private async waitForActivity(
    timeoutMs: number,
    signal?: AbortSignal,
    sinceVersion = this.activityVersion,
  ): Promise<boolean> {
    const timeout = Math.min(Math.max(timeoutMs, 1_000), MAX_TEAM_WAIT_MS);
    // Activity that landed between the caller deciding to wait and this
    // registration must not be lost, or the wait blocks for its full timeout.
    if (this.activityVersion !== sinceVersion) return true;
    return await new Promise<boolean>((resolve) => {
      let settled = false;
      const finish = (activity: boolean) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        this.waiters.delete(onActivity);
        signal?.removeEventListener("abort", onAbort);
        resolve(activity);
      };
      const onActivity = () => finish(true);
      const onAbort = () => finish(false);
      const timer = setTimeout(() => finish(false), timeout);
      this.waiters.add(onActivity);
      signal?.addEventListener("abort", onAbort, { once: true });
    });
  }

  private listText(): string {
    const now = Date.now();
    const lines = [`• [ROOT] ${ROOT_PATH}`];
    for (const agent of [...this.agents.values()].sort((a, b) => a.path.localeCompare(b.path))) {
      const preset = agent.team ? ` [${agent.team}]` : "";
      const quiet = this.runtime.formatDuration(Math.max(now - agent.lastActivityAt, 0));
      const detail = agent.report
        ? `${agent.report.state}${agent.report.question ? `: ${agent.report.question}` : ` — ${agent.report.summary}`}`
        : agent.description;
      lines.push(`• [${this.status(agent).toUpperCase()}] ${agent.path} (${agent.role})${preset} — ${detail} — quiet ${quiet}`);
    }
    return `Team agents (${this.agents.size + 1}):\n${lines.join("\n")}`;
  }

  /**
   * V2 control-plane tools.
   *
   * `promptSnippet` here only reaches the root session's system prompt. A child
   * session runs with `systemPromptOverride`, and Pi then renders the preamble
   * alone: the `tools` and `rules` sections are skipped, so snippets and
   * `promptGuidelines` never reach a child. Everything a child must know about
   * these tools therefore lives in `usageHint` and in each tool's `description`.
   */
  private controlTools(senderPath: string): ToolDefinition[] {
    const spawnTool = defineTool({
      name: "spawn_team_agent",
      label: "Spawn Team Agent",
      description: "Spawn a Subagents V2 child agent under your current /root/... path. The child starts asynchronously and can communicate with the team.",
      promptSnippet: "Spawn a named V2 child agent with a stable /root/... path.",
      executionMode: "parallel",
      parameters: Type.Object({
        task_name: Type.String({ description: "Lowercase path segment for the child, e.g. researcher or verifier_2." }),
        message: Type.String({ description: "Self-contained initial task for the child." }),
        agent_type: Type.Optional(Type.String({ description: "External agent definition/profile name. Defaults to general-purpose." })),
        description: Type.Optional(Type.String({ description: "Short 3-5 word label shown in the team tree and subagent UI. Defaults to task_name." })),
        model: Type.Optional(Type.String({ description: "Optional model allowed by the selected external agent definition." })),
        max_turns: Type.Optional(Type.Integer({ minimum: 0, description: "Optional soft turn cap; external agent definition takes precedence." })),
        thinking_level: Type.Optional(Type.String({ description: "Optional thinking level: off, minimal, low, medium, high, xhigh, or max. Clamped to what the model supports; omitted inherits Pi settings." })),
        capability_mode: Type.Optional(Type.String({ description: "Tool access for the child: read-only or all (legacy execute/read-write map to all). Defaults to the agent definition, then all." })),
      }),
      execute: async (toolCallId, params, signal, _onUpdate, ctx) => {
        await this.flushBufferedMessages();
        const agent = await this.spawnAgent(senderPath, params, toolCallId, signal, ctx);
        return toolText(`Spawned ${agent.path} (${agent.role}). Final output will be delivered automatically to ${senderPath}.`, {
          agentPath: agent.path,
          parentPath: senderPath,
          subagentId: agent.record.id,
        });
      },
    });

    const sendTool = defineTool({
      name: "team_send_message",
      label: "Send Team Message",
      description:
        "Send a semantic message to a V2 agent without starting an idle recipient turn. " +
        "Consecutive messages from one sender to one target merge into a single steer and are delivered before your next team action or when your turn ends.",
      promptSnippet: "Send a queue-only message to /root or another V2 agent.",
      parameters: Type.Object({
        target: Type.String({ description: "Absolute /root/... path. A relative name addresses your own child." }),
        message: Type.String({ description: "Message payload." }),
      }),
      execute: async (_toolCallId, params) => {
        const target = this.resolveTarget(senderPath, params.target);
        const message = requireText(params.message, "message");
        const buffered = this.bufferMessage(senderPath, target, message);
        // Buffering is still team activity: a waiting peer must wake even though
        // the payload leaves on the next flush.
        this.notifyActivity();
        return toolText(
          `Buffered MESSAGE for ${target} (${buffered} pending from ${senderPath}). It merges with your other messages to that agent and is delivered before your next team action or when this turn ends.`,
          { sender: senderPath, target, buffered },
        );
      },
    });

    const followupTool = defineTool({
      name: "team_followup_task",
      label: "Follow Up Team Task",
      description: "Send a NEW_TASK to a V2 agent. It triggers an idle recipient turn or queues after a running recipient's current work.",
      promptSnippet: "Assign a follow-up task that triggers the recipient agent.",
      parameters: Type.Object({
        target: Type.String({ description: "Absolute /root/... path. A relative name addresses your own child." }),
        message: Type.String({ description: "New task or follow-up instruction." }),
      }),
      execute: async (toolCallId, params, signal) => {
        await this.flushBufferedMessages();
        const targetPath = this.resolveTarget(senderPath, params.target);
        const message = requireText(params.message, "message");
        if (targetPath === ROOT_PATH) {
          await this.deliver(senderPath, targetPath, "NEW_TASK", message, true);
          return toolText(`Delivered NEW_TASK from ${senderPath} to ${targetPath}.`, { sender: senderPath, target: targetPath, reactivated: false });
        }
        const target = this.agentForPath(targetPath);
        if (!target.record.finished) {
          if (this.runtime.backgroundState(target.record) === "queued") {
            // The parent explicitly asked this queued agent for work, so its
            // pending run moves ahead of unrelated launches instead of waiting.
            const promoted = this.runtime.prioritizeBackground(target.record);
            target.pendingTasks.push({ senderPath, message, toolCallId, kind: "NEW_TASK" });
            this.notifyActivity();
            return toolText(
              promoted
                ? `Queued NEW_TASK for ${targetPath} and promoted its pending run to the front of the concurrency queue.`
                : `Queued NEW_TASK for ${targetPath}; it will run after the agent's already-queued work.`,
              { sender: senderPath, target: targetPath, reactivated: false, queued: true, promoted },
            );
          }
          await this.deliver(senderPath, targetPath, "NEW_TASK", message, true);
          return toolText(`Queued NEW_TASK from ${senderPath} to running agent ${targetPath}.`, { sender: senderPath, target: targetPath, reactivated: false });
        }
        const current = this.status(target);
        if (!this.canReceiveMessage(current)) {
          throw new Error(`team agent ${targetPath} cannot be reactivated after ${current}`);
        }
        const scheduled = await this.reactivateAgent(target, senderPath, "NEW_TASK", message, toolCallId, signal);
        return toolText(`Reactivated ${targetPath} with NEW_TASK while preserving its child-session history.`, {
          sender: senderPath,
          target: targetPath,
          reactivated: true,
          queued: scheduled === "queued",
          subagentId: target.record.id,
        });
      },
    });

    const waitTool = defineTool({
      name: "team_wait",
      label: "Wait for Team Activity",
      description: "Wait for V2 team activity while another child agent is active. Root never blocks here: it returns immediately so background agents can continue and FINAL_ANSWER can reactivate the root turn.",
      promptSnippet: "Root must not park on team_wait; children wait only while another V2 agent is active, otherwise finish the turn and become IDLE.",
      parameters: Type.Object({
        timeout_ms: Type.Optional(Type.Integer({ minimum: 1_000, maximum: MAX_TEAM_WAIT_MS, description: "Wait timeout in milliseconds. Defaults to 120000." })),
      }),
      execute: async (_toolCallId, params, signal) => {
        // Recorded before the flush so activity that lands during the flush is
        // still observed by the wait below.
        const enteredAt = this.activityVersion;
        await this.flushBufferedMessages();
        if (senderPath === ROOT_PATH) {
          return toolText(`Root does not block on team_wait. Finish this turn while background agents continue; FINAL_ANSWER will reactivate /root when results arrive.\n\n${this.listText()}`, {
            activity: false,
            idle: true,
            root: true,
            version: this.activityVersion,
          });
        }
        if (!this.hasActivePeer(senderPath)) {
          return toolText(`No other V2 agent is active. Finish this turn to become IDLE; a future team_followup_task can reactivate this child session.\n\n${this.listText()}`, {
            activity: false,
            idle: true,
            version: this.activityVersion,
          });
        }
        const activity = await this.waitForActivity(params.timeout_ms ?? DEFAULT_TEAM_WAIT_MS, signal, enteredAt);
        const idle = !this.hasActivePeer(senderPath);
        const status = idle
          ? "No other V2 agent is active now. Finish this turn to become IDLE; a future team_followup_task can reactivate this child session."
          : activity
            ? "Team activity observed."
            : "Team wait timed out or was interrupted.";
        return toolText(`${status}\n\n${this.listText()}`, {
          activity,
          idle,
          version: this.activityVersion,
        });
      },
    });

    const listTool = defineTool({
      name: "team_list",
      label: "List Team Agents",
      description: "List the Subagents V2 team tree with stable paths, roles, and status.",
      promptSnippet: "Inspect the current V2 team tree and statuses.",
      parameters: Type.Object({}),
      execute: async () => {
        await this.flushBufferedMessages();
        return toolText(this.listText(), {
          agents: [...this.agents.values()].map((agent) => ({
            path: agent.path,
            parentPath: agent.parentPath,
            role: agent.role,
            status: this.status(agent),
            report: agent.report,
            quietMs: Date.now() - agent.lastActivityAt,
          })),
        });
      },
    });

    const interruptTool = defineTool({
      name: "team_interrupt",
      label: "Interrupt Team Agent",
      description: "Abort a running V2 agent by stable /root/... path.",
      promptSnippet: "Interrupt a running V2 agent.",
      parameters: Type.Object({ target: Type.String({ description: "Absolute /root/... path of the running agent." }) }),
      execute: async (_toolCallId, params) => {
        await this.flushBufferedMessages();
        const targetPath = this.resolveTarget(senderPath, params.target);
        if (targetPath === ROOT_PATH) throw new Error("team_interrupt cannot abort /root");
        const target = this.agentForPath(targetPath);
        if (target.record.finished) return toolText(`${targetPath} already finished (${this.status(target)}).`, { target: targetPath, finished: true });
        this.runtime.cancel(target.record);
        this.touch(target);
        this.notifyActivity();
        return toolText(`Interrupt requested for ${targetPath}.`, { target: targetPath, finished: false });
      },
    });

    const reportTool = defineTool({
      name: "team_report",
      label: "Report Team Result",
      description:
        "Declare how this turn ends so your parent is woken only when it must act. " +
        "Call it once before finishing. waiting means your own children are pending: end this turn to release your slot and be resumed by their results. Without a report your final text reaches the parent unconditionally.",
      promptSnippet: "Declare ready / waiting / blocked / failed / dropped before ending your turn; waiting releases the run after turn end, dropped is a no-op result.",
      parameters: Type.Object({
        state: Type.Union([
          Type.Literal("ready"),
          Type.Literal("waiting"),
          Type.Literal("blocked"),
          Type.Literal("failed"),
          Type.Literal("dropped"),
        ], { description: "ready: the requested result is deliverable. waiting: your own children are pending and no local work remains; end the turn. blocked: you need a parent decision to continue. failed: the work cannot be completed. dropped: the parent has nothing to act on." }),
        summary: Type.String({ description: "One sentence the parent can act on, not a work report. Your final text is ignored when you report." }),
        question: Type.Optional(Type.String({ description: "Required with blocked: the single concrete decision the parent must make." })),
      }),
      execute: async (_toolCallId, params) => {
        await this.flushBufferedMessages();
        const agent = this.agentForPath(senderPath);
        const state = params.state;
        const summary = requireText(params.summary, "summary");
        const question = params.question?.trim() || undefined;
        if (state === "blocked" && !question) {
          throw new Error("team_report with state=blocked requires the question the parent must answer");
        }
        if (state === "waiting" && !this.hasPendingChild(senderPath)) {
          throw new Error("team_report with state=waiting requires your own pending child; handle completed results or report blocked for a parent decision");
        }
        agent.report = { state, summary, ...(question ? { question } : {}) };
        this.touch(agent);
        return toolText(
          state === "dropped"
            ? `Recorded dropped. ${senderPath} requests no parent action; a waiting parent still receives this dependency completion. team_list keeps this summary.`
            : state === "waiting"
              ? `Recorded waiting. End this turn to release your execution slot; a child result will resume ${senderPath}. Your parent is not notified of this intermediate state.`
              : `Recorded ${state}. Your parent is notified when this turn ends.`,
          { agentPath: senderPath, state, summary, ...(question ? { question } : {}) },
        );
      },
    });

    const tools = [spawnTool, sendTool, followupTool, waitTool, listTool, interruptTool];
    // The root has no parent, so a result report would be dead weight in its tool list.
    if (senderPath !== ROOT_PATH) tools.push(reportTool);
    return tools;
  }

  private spawnTeamTool(): ToolDefinition {
    return defineTool({
      name: "spawn_team",
      label: "Spawn Team Preset",
      description: "Launch an external Subagents V2 team preset from project/global/bundled configuration. Members start asynchronously and can communicate by stable agent paths.",
      promptSnippet: "Launch a configured V2 team preset for collaborative work.",
      executionMode: "parallel",
      parameters: Type.Object({
        team: Type.String({ description: "Team preset name as listed by /subagent-teams, e.g. research, implementation, or review." }),
        task: Type.String({ description: "Shared team objective substituted into member task templates as {{task}}." }),
        members: Type.Optional(Type.Array(Type.String(), { description: "Optional member subset to start. Omitted starts the full preset roster. Preset order is preserved." })),
        model: Type.Optional(Type.String({ description: "Optional model applied to every selected member, overriding each member's preset model. Must be enabled for the member's agent definition." })),
        thinking_level: Type.Optional(Type.String({ description: "Optional thinking level applied to every selected member, overriding preset thinking levels. Clamped per model." })),
        max_turns: Type.Optional(Type.Integer({ minimum: 0, description: "Optional soft turn cap applied to every selected member; 0 means unlimited. Agent definitions still take precedence." })),
        capability_mode: Type.Optional(Type.String({ description: "Tool access for every selected member: read-only or all (legacy execute/read-write map to all). Defaults to each agent definition, then all." })),
        instructions: Type.Optional(Type.String({ description: "Extra team-wide instruction appended to the preset instructions and shared with every member's system prompt." })),
        timeout_ms: Type.Optional(Type.Integer({ minimum: 0, maximum: MAX_TEAM_WAIT_MS, description: "Wait this long for members to finish and return their results. 0 or omitted returns immediately and delivers final answers to /root asynchronously." })),
      }),
      execute: async (toolCallId, params, signal, _onUpdate, ctx) => {
        const team = selectedTeam(ctx.cwd, requireText(params.team, "team"));
        const task = requireText(params.task, "task");
        const members = selectTeamMembers(team, params.members);
        const paths = members.map((member) => childPath(ROOT_PATH, member.name));
        for (const path of paths) {
          if (this.agents.has(path) || this.reservedPaths.has(path)) throw new Error(`team agent path already exists: ${path}`);
        }
        const roster = members.map((member, index) => `${paths[index]} (${member.agent})`).join("\n");
        const systemPromptExtra = [
          `Team preset: ${team.name}. Objective: ${task}`,
          team.instructions ?? "",
          params.instructions?.trim() ?? "",
          `Team roster:\n${roster}`,
        ].filter(Boolean).join("\n\n");
        const spawned: TeamAgent[] = [];
        try {
          for (const [index, member] of members.entries()) {
            const path = paths[index];
            const message = renderTeamTemplate(member.task, {
              task,
              team: team.name,
              agentPath: path,
              parentPath: ROOT_PATH,
            });
            spawned.push(await this.spawnAgent(
              ROOT_PATH,
              {
                task_name: member.name,
                message,
                agent_type: member.agent,
                description: member.description,
                model: params.model ?? member.model,
                max_turns: params.max_turns ?? member.maxTurns,
                thinking_level: params.thinking_level ?? member.thinkingLevel,
                capability_mode: params.capability_mode,
              },
              // The real tool-call id is recorded on the child run for history.
              // Parent task metadata is omitted below because one call starts
              // many members and has no single parent task id.
              toolCallId,
              signal,
              ctx,
              {
                team: team.name,
                systemPromptExtra,
                description: member.description ?? member.name,
                deferStart: true,
                // One call starts many members, so there is no single parent
                // task id. Empty means: keep spawn_team's own tool-call metadata.
                parentToolCallId: "",
              },
            ));
          }
        } catch (error) {
          this.rollbackTeam(spawned, `Rolled back partial team ${team.name} startup.`);
          throw error;
        }
        if (signal?.aborted) {
          this.rollbackTeam(spawned, `Rolled back team ${team.name}: the spawn request was cancelled before members started.`);
          throw new Error(`spawn_team for ${team.name} was cancelled before any member started`);
        }
        for (const agent of spawned) this.runtime.scheduleBackground(agent.record, agent.record.prompt);
        const header = [
          `Spawned team ${team.name} [${team.scope}] for: ${task}`,
          ...spawned.map((agent) => `• ${agent.path} (${agent.role})`),
        ].join("\n");
        const subsetNote = members.length === team.members.length
          ? ""
          : `\nStarted ${members.length} of ${team.members.length} preset members; member task templates may reference agents that were not started.`;
        const details = {
          team: team.name,
          scope: team.scope,
          agents: spawned.map((agent) => ({ path: agent.path, subagentId: agent.record.id, role: agent.role })),
        };
        const timeoutMs = params.timeout_ms ?? 0;
        if (timeoutMs <= 0) {
          return toolText(`${header}${subsetNote}\nFinal answers will be delivered automatically to /root.`, details);
        }
        await this.runtime.waitForRecords(spawned.map((agent) => agent.record.id), timeoutMs, signal);
        const results = spawned.map((agent) => this.runtime.formatSubagentResult(agent.record)).join("\n\n---\n\n");
        return toolText(`${header}${subsetNote}\n\n${results}`, details);
      },
    });
  }
}

export function registerV2Tools(pi: ExtensionAPI, runtime: SubagentRuntime): void {
  const coordinator = new TeamCoordinator(pi, runtime);
  coordinator.register();
  // A root turn has no `handleFinished`; its end is the only reliable flush point
  // for messages buffered by the last team action of the turn.
  pi.on("turn_end", async () => {
    try {
      await coordinator.flushBufferedMessages();
    } catch {
      // A target that ended mid-turn cannot accept the payload; the merge already
      // saved the context cost, and the tool result reported the buffer count.
    }
  });
}

export { CONTROL_TOOL_NAMES, TEAM_MESSAGE_TYPE };
