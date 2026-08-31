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
import { loadTeamDefinitions, renderTeamTemplate, selectedTeam, type TeamDefinition } from "./teams.ts";

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
] as const;

type TeamMessageKind = "MESSAGE" | "NEW_TASK" | "FINAL_ANSWER";
type TeamAgentStatus = "queued" | "running" | "idle" | "failed" | "cancelled";

type TeamAgent = {
  path: string;
  parentPath: string;
  taskName: string;
  role: string;
  description: string;
  record: SubagentRecord;
  status: TeamAgentStatus;
  team?: string;
  pendingTasks: Array<{ senderPath: string; message: string; toolCallId: string; kind: "NEW_TASK" | "FINAL_ANSWER" }>;
};

type SpawnAgentArgs = {
  task_name: string;
  message: string;
  agent_type?: string;
  model?: string;
  max_turns?: number;
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
      description: "List external Subagents V2 team presets",
      handler: async (_args, ctx) => {
        const teams = [...loadTeamDefinitions(ctx.cwd).values()].sort((a, b) => a.name.localeCompare(b.name));
        if (teams.length === 0) {
          ctx.ui.notify("No Subagents V2 team presets were found.", "warning");
          return;
        }
        const lines = teams.map((team) => {
          const members = team.members.map((member) => `${member.name}:${member.agent}`).join(", ") || "disabled";
          return `${team.enabled ? "✓" : "×"} ${team.name} [${team.scope}] — ${team.description} — ${members}`;
        });
        ctx.ui.notify(lines.join("\n"), "info");
      },
    });
  }

  private notifyActivity(): void {
    this.activityVersion += 1;
    for (const resolve of this.waiters) resolve(this.activityVersion);
    this.waiters.clear();
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
    if (agent.record.terminalStatus === "completed") return "idle";
    return agent.record.terminalStatus ?? agent.status;
  }

  private usageHint(path: string, parentPath: string, extra?: string): string {
    return [
      `You are ${path}, an agent in a grok-pi Subagents V2 team. Your parent is ${parentPath}.`,
      "All team agents share the same working directory and can see each other's filesystem changes.",
      "Use spawn_team_agent to create a nested child, team_send_message for a queue-only semantic message, team_followup_task to trigger a new recipient task, team_wait only while another agent is actively running, team_list to inspect the team tree, and team_interrupt to stop a running agent.",
      "When no other agent is active, finish your turn instead of waiting; you become IDLE and team_followup_task can reactivate this same child session later.",
      "Use absolute /root/... paths when messaging siblings. A final response from your turn is automatically delivered to your parent as FINAL_ANSWER.",
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
      if (target.record.finished && targetStatus !== "idle") {
        throw new Error(`team agent ${targetPath} cannot receive messages after ${targetStatus}`);
      }
      if (target.record.finished && triggerTurn) {
        throw new Error(`team agent ${targetPath} is idle; use team_followup_task to reactivate it`);
      }
      await target.record.session.sendCustomMessage(
        { customType: TEAM_MESSAGE_TYPE, content, display: true, details },
        { triggerTurn, deliverAs: triggerTurn ? "followUp" : "steer" },
      );
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
    if (this.status(target) !== "idle") {
      throw new Error(`team agent ${target.path} cannot be reactivated from ${this.status(target)}`);
    }
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
    this.notifyActivity();
    const output = this.runtime.finalOutput(record);
    const payload = status === "completed"
      ? output || "Agent completed without text output."
      : `Agent ${status}.${error ? ` Error: ${error}` : ""}${output ? `\n\nLast output:\n${output}` : ""}`;
    try {
      const parent = agent.parentPath === ROOT_PATH ? undefined : this.agents.get(agent.parentPath);
      const parentStatus = parent ? this.status(parent) : undefined;
      if (parent && parentStatus === "idle") {
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

  private async spawnAgent(
    senderPath: string,
    args: SpawnAgentArgs,
    toolCallId: string,
    signal: AbortSignal | undefined,
    ctx: ExtensionContext,
    options: { team?: string; systemPromptExtra?: string; description?: string; deferStart?: boolean } = {},
  ): Promise<TeamAgent> {
    validateAgentPath(senderPath);
    if (senderPath !== ROOT_PATH && !this.agents.has(senderPath)) throw new Error(`unknown spawning agent: ${senderPath}`);
    const taskName = pathSegment(requireText(args.task_name, "task_name"));
    const message = requireText(args.message, "message");
    const path = childPath(senderPath, taskName);
    if (this.agents.has(path) || this.reservedPaths.has(path)) throw new Error(`team agent path already exists: ${path}`);
    this.reservedPaths.add(path);
    try {
      const customTools = this.controlTools(path);
      const record = await this.runtime.createRecord(
        toolCallId,
        {
          prompt: message,
          description: options.description ?? taskName,
          subagent_type: args.agent_type ?? "general-purpose",
          background: true,
          model: args.model,
          max_turns: args.max_turns,
        },
        signal,
        ctx,
        {
          customTools,
          systemPromptSuffix: this.usageHint(path, senderPath, options.systemPromptExtra),
          onFinished: (finishedRecord, status, error) => this.handleFinished(path, finishedRecord, status, error),
        },
      );
      const agent: TeamAgent = {
        path,
        parentPath: senderPath,
        taskName,
        role: args.agent_type ?? "general-purpose",
        description: options.description ?? taskName,
        record,
        status: "running",
        team: options.team,
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

  private hasActivePeer(senderPath: string): boolean {
    return [...this.agents.values()].some((agent) => {
      if (agent.path === senderPath) return false;
      const status = this.status(agent);
      return status === "running" || status === "queued";
    });
  }

  private async waitForActivity(timeoutMs: number, signal?: AbortSignal): Promise<boolean> {
    const timeout = Math.min(Math.max(timeoutMs, 1_000), MAX_TEAM_WAIT_MS);
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
    const lines = [`• [ROOT] ${ROOT_PATH}`];
    for (const agent of [...this.agents.values()].sort((a, b) => a.path.localeCompare(b.path))) {
      lines.push(`• [${this.status(agent).toUpperCase()}] ${agent.path} (${agent.role}) — ${agent.description}`);
    }
    return `Team agents (${this.agents.size + 1}):\n${lines.join("\n")}`;
  }

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
        model: Type.Optional(Type.String({ description: "Optional model allowed by the selected external agent definition." })),
        max_turns: Type.Optional(Type.Integer({ minimum: 0, description: "Optional soft turn cap; external agent definition takes precedence." })),
      }),
      execute: async (toolCallId, params, signal, _onUpdate, ctx) => {
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
      description: "Send a semantic message to a V2 agent. This does not start an idle recipient turn; a running recipient receives it through the steer queue.",
      promptSnippet: "Send a queue-only message to /root or another V2 agent.",
      parameters: Type.Object({
        target: Type.String({ description: "Absolute /root/... path. A relative name addresses your own child." }),
        message: Type.String({ description: "Message payload." }),
      }),
      execute: async (_toolCallId, params) => {
        const target = this.resolveTarget(senderPath, params.target);
        const message = requireText(params.message, "message");
        await this.deliver(senderPath, target, "MESSAGE", message, false);
        return toolText(`Delivered MESSAGE from ${senderPath} to ${target} without forcing an idle turn.`, { sender: senderPath, target });
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
        const targetPath = this.resolveTarget(senderPath, params.target);
        const message = requireText(params.message, "message");
        if (targetPath === ROOT_PATH) {
          await this.deliver(senderPath, targetPath, "NEW_TASK", message, true);
          return toolText(`Delivered NEW_TASK from ${senderPath} to ${targetPath}.`, { sender: senderPath, target: targetPath, reactivated: false });
        }
        const target = this.agentForPath(targetPath);
        if (!target.record.finished) {
          if (this.runtime.backgroundState(target.record) === "queued") {
            target.pendingTasks.push({ senderPath, message, toolCallId, kind: "NEW_TASK" });
            this.notifyActivity();
            return toolText(`Queued NEW_TASK for ${targetPath}; it will run after the agent's already-queued work.`, {
              sender: senderPath,
              target: targetPath,
              reactivated: false,
              queued: true,
            });
          }
          await this.deliver(senderPath, targetPath, "NEW_TASK", message, true);
          return toolText(`Queued NEW_TASK from ${senderPath} to running agent ${targetPath}.`, { sender: senderPath, target: targetPath, reactivated: false });
        }
        if (this.status(target) !== "idle") {
          throw new Error(`team agent ${targetPath} cannot be reactivated after ${this.status(target)}`);
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
        const activity = await this.waitForActivity(params.timeout_ms ?? DEFAULT_TEAM_WAIT_MS, signal);
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
      execute: async () => toolText(this.listText(), {
        agents: [...this.agents.values()].map((agent) => ({ path: agent.path, parentPath: agent.parentPath, role: agent.role, status: this.status(agent) })),
      }),
    });

    const interruptTool = defineTool({
      name: "team_interrupt",
      label: "Interrupt Team Agent",
      description: "Abort a running V2 agent by stable /root/... path.",
      promptSnippet: "Interrupt a running V2 agent.",
      parameters: Type.Object({ target: Type.String({ description: "Absolute /root/... path of the running agent." }) }),
      execute: async (_toolCallId, params) => {
        const targetPath = this.resolveTarget(senderPath, params.target);
        if (targetPath === ROOT_PATH) throw new Error("team_interrupt cannot abort /root");
        const target = this.agentForPath(targetPath);
        if (target.record.finished) return toolText(`${targetPath} already finished (${this.status(target)}).`, { target: targetPath, finished: true });
        this.runtime.cancel(target.record);
        this.notifyActivity();
        return toolText(`Interrupt requested for ${targetPath}.`, { target: targetPath, finished: false });
      },
    });

    return [spawnTool, sendTool, followupTool, waitTool, listTool, interruptTool];
  }

  private spawnTeamTool(): ToolDefinition {
    return defineTool({
      name: "spawn_team",
      label: "Spawn Team Preset",
      description: "Launch an external Subagents V2 team preset from project/global/bundled configuration. Members start asynchronously and can communicate by stable agent paths.",
      promptSnippet: "Launch a configured V2 team preset for collaborative work.",
      executionMode: "parallel",
      parameters: Type.Object({
        team: Type.String({ description: "Team preset name, e.g. research, implementation, or review." }),
        task: Type.String({ description: "Shared team objective substituted into member task templates as {{task}}." }),
      }),
      execute: async (toolCallId, params, signal, _onUpdate, ctx) => {
        const team = selectedTeam(ctx.cwd, requireText(params.team, "team"));
        const task = requireText(params.task, "task");
        const paths = team.members.map((member) => childPath(ROOT_PATH, member.name));
        for (const path of paths) {
          if (this.agents.has(path) || this.reservedPaths.has(path)) throw new Error(`team agent path already exists: ${path}`);
        }
        const roster = team.members.map((member, index) => `${paths[index]} (${member.agent})`).join("\n");
        const systemPromptExtra = [
          `Team preset: ${team.name}. Objective: ${task}`,
          team.instructions ?? "",
          `Team roster:\n${roster}`,
        ].filter(Boolean).join("\n\n");
        const spawned: TeamAgent[] = [];
        try {
          for (const [index, member] of team.members.entries()) {
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
                model: member.model,
                max_turns: member.maxTurns,
              },
              `${toolCallId}:${member.name}`,
              signal,
              ctx,
              { team: team.name, systemPromptExtra, description: member.description ?? member.name, deferStart: true },
            ));
          }
        } catch (error) {
          for (const agent of spawned) {
            this.agents.delete(agent.path);
            this.runtime.discard(agent.record, `Rolled back partial team ${team.name} startup.`);
          }
          this.notifyActivity();
          throw error;
        }
        for (const agent of spawned) this.runtime.scheduleBackground(agent.record, agent.record.prompt);
        return toolText(
          `Spawned team ${team.name} [${team.scope}] for: ${task}\n${spawned.map((agent) => `• ${agent.path} (${agent.role})`).join("\n")}\nFinal answers will be delivered automatically to /root.`,
          { team: team.name, scope: team.scope, agents: spawned.map((agent) => ({ path: agent.path, subagentId: agent.record.id, role: agent.role })) },
        );
      },
    });
  }
}

export function registerV2Tools(pi: ExtensionAPI, runtime: SubagentRuntime): void {
  const coordinator = new TeamCoordinator(pi, runtime);
  coordinator.register();
}

export { CONTROL_TOOL_NAMES, TEAM_MESSAGE_TYPE };
