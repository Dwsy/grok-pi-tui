/** Shared Pi child-session runtime used by the V1 and optional V2 tool surfaces. */

import { dirname, join } from "node:path";
import {
  createAgentSession,
  DefaultResourceLoader,
  getAgentDir,
  SessionManager,
  SettingsManager,
  type AgentSessionEvent,
  type ExtensionAPI,
  type ExtensionContext,
  type ToolDefinition,
} from "@earendil-works/pi-coding-agent";
import {
  CAPABILITY_TOOLS,
  MAX_BACKGROUND_CONCURRENCY,
  MAX_WAIT_MS,
  SHORT_SUBAGENT_ID_LENGTH,
  extractUsage,
  newSubagentId,
  requireText,
  textFromContent,
} from "./shared.ts";
import { profileFor, selectedDefinition } from "./definitions.ts";
import {
  appendPersistedRecord,
  createBridgeEmitter,
  latestPersistedRecords,
  persist,
  persistedRunBranch,
  shortSubagentIdFor,
  subagentStateFile,
  type BridgeEmitter,
  type PersistedRecord,
  type SubagentRecord,
} from "./bridge.ts";

export type SpawnParams = {
  prompt: string;
  description: string;
  subagent_type?: string;
  background?: boolean;
  capability_mode?: string;
  model?: string;
  max_turns?: number;
};

export type MessageDelivery = "steer" | "follow_up";

export type RecordFinishHandler = (
  record: SubagentRecord,
  status: "completed" | "failed" | "cancelled",
  error?: string,
) => void | Promise<void>;

export type RecordCreateOptions = {
  customTools?: ToolDefinition[];
  systemPromptSuffix?: string;
  onFinished?: RecordFinishHandler;
};

function lastAssistantText(session: SubagentRecord["session"]): string {
  for (let index = session.messages.length - 1; index >= 0; index -= 1) {
    const message = session.messages[index];
    if (message.role !== "assistant") continue;
    return message.content
      .filter((block) => block.type === "text")
      .map((block) => block.text)
      .join("")
      .trim();
  }
  return "";
}

function childUpdate(
  event: AgentSessionEvent,
  toolArgs: Map<string, unknown>,
): Record<string, unknown> | undefined {
  if (event.type === "message_update") {
    if (event.assistantMessageEvent.type === "text_delta") {
      return { type: "assistant_delta", text: event.assistantMessageEvent.delta };
    }
    if (event.assistantMessageEvent.type === "thinking_delta") {
      return { type: "thinking_delta", text: event.assistantMessageEvent.delta };
    }
  }
  if (event.type === "message_end" && event.message.role === "user") {
    const text = textFromContent(event.message.content);
    return text ? { type: "user", text } : undefined;
  }
  if (event.type === "tool_execution_start") {
    toolArgs.set(event.toolCallId, event.args);
    return { type: "tool_call", toolCallId: event.toolCallId, toolName: event.toolName, args: event.args };
  }
  if (event.type === "tool_execution_update") {
    toolArgs.set(event.toolCallId, event.args);
    return { type: "tool_update", toolCallId: event.toolCallId, toolName: event.toolName, args: event.args, partialResult: event.partialResult };
  }
  if (event.type === "tool_execution_end") {
    const args = toolArgs.get(event.toolCallId);
    toolArgs.delete(event.toolCallId);
    return { type: "tool_result", toolCallId: event.toolCallId, toolName: event.toolName, args, result: event.result, isError: event.isError };
  }
  return undefined;
}

export class SubagentRuntime {
  readonly records = new Map<string, SubagentRecord>();
  private readonly queuedBackground: Array<{ record: SubagentRecord; run: () => Promise<void> }> = [];
  private readonly finishHandlers = new Map<string, RecordFinishHandler>();
  private readonly runningBackgroundIds = new Set<string>();
  private readonly emit: BridgeEmitter;
  private readonly pi: ExtensionAPI;
  private readonly persistRecord: typeof persist;
  private runningBackground = 0;

  constructor(
    pi: ExtensionAPI,
    emit: BridgeEmitter = createBridgeEmitter(),
    persistRecord: typeof persist = persist,
  ) {
    this.pi = pi;
    this.emit = emit;
    this.persistRecord = persistRecord;
  }

  onSessionStart(_ctx: ExtensionContext): void {
    this.publishTodoBacking();
  }

  async replayPersisted(
    ctx: ExtensionContext,
    mode: "load" | "recovery" = "load",
    requestId?: string,
  ): Promise<void> {
    await this.emit.ready;
    try {
      // session/load owns full transcript replay. Crash recovery only settles
      // running orphans, because the existing Pager view already owns prior text.
      const parentSessionFile = ctx.sessionManager.getSessionFile();
      const stateFile = parentSessionFile ? subagentStateFile(parentSessionFile) : undefined;
      for (const snapshot of latestPersistedRecords(ctx)) {
        if (mode === "recovery" && snapshot.status !== "running") continue;
        const replay = mode === "load";
        this.emit(snapshot, "spawned", {
          parentToolCallId: snapshot.parentToolCallId,
          description: snapshot.description,
          subagentType: snapshot.type,
          background: snapshot.background,
          capabilityMode: snapshot.capabilityMode,
          model: snapshot.modelId,
          prompt: snapshot.prompt,
          reconcile: mode === "recovery",
        }, replay);
        if (mode === "recovery") {
          const error = "Pi host restarted before child completion";
          this.emit(snapshot, "finished", {
            status: "cancelled",
            durationMs: Math.max(0, Date.now() - snapshot.startedAt),
            turns: snapshot.turnCount,
            toolCalls: snapshot.toolCallCount,
            tokensUsed: snapshot.tokensUsed,
            error,
          });
          if (stateFile) {
            appendPersistedRecord(stateFile, {
              ...snapshot,
              status: "cancelled",
              lastError: error,
            });
          }
          continue;
        }
        const transcriptError = this.replayChildTranscript(snapshot);
        if (transcriptError) {
          this.emit(snapshot, "finished", {
            status: "failed",
            durationMs: Math.max(0, Date.now() - snapshot.startedAt),
            turns: snapshot.turnCount,
            toolCalls: snapshot.toolCallCount,
            tokensUsed: snapshot.tokensUsed,
            error: transcriptError,
          }, true);
          continue;
        }
        const live = this.records.get(snapshot.id);
        if (snapshot.status === "running" && live && !live.finished) continue;
        const status = snapshot.status === "running" ? "cancelled" : snapshot.status;
        this.emit(snapshot, "finished", {
          status,
          durationMs: Math.max(0, Date.now() - snapshot.startedAt),
          turns: snapshot.turnCount,
          toolCalls: snapshot.toolCallCount,
          tokensUsed: snapshot.tokensUsed,
          error: snapshot.status === "running" ? "Pi host restarted before child completion" : snapshot.lastError,
        }, true);
      }
      this.publishTodoBacking();
    } finally {
      if (requestId) {
        const marker = `replay-${requestId}`;
        this.emit(
          { id: marker, childSessionId: marker, parentSessionId: ctx.sessionManager.getSessionId() },
          "replay_complete",
          { requestId },
          mode === "load",
        );
      }
    }
  }

  private replayChildTranscript(snapshot: PersistedRecord): string | undefined {
    let entries: readonly unknown[];
    try {
      entries = persistedRunBranch(snapshot);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return `child transcript is unavailable: ${message}`;
    }
    const toolArgs = new Map<string, unknown>();
    for (const entry of entries) {
      const message = (entry as { message?: unknown }).message;
      if (typeof message !== "object" || message === null) continue;
      const child = message as { role?: unknown; content?: unknown; toolCallId?: unknown; toolName?: unknown; isError?: unknown };
      if (child.role === "user") {
        const text = textFromContent(child.content);
        if (text) this.emit(snapshot, "child_update", { update: { type: "user", text } }, true);
        continue;
      }
      if (child.role === "assistant" && Array.isArray(child.content)) {
        for (const block of child.content) {
          if (typeof block !== "object" || block === null) continue;
          const value = block as { type?: unknown; text?: unknown; thinking?: unknown; id?: unknown; name?: unknown; arguments?: unknown };
          if (value.type === "text" && typeof value.text === "string" && value.text) {
            this.emit(snapshot, "child_update", { update: { type: "assistant_delta", text: value.text } }, true);
          } else if (value.type === "thinking" && typeof value.thinking === "string" && value.thinking) {
            this.emit(snapshot, "child_update", { update: { type: "thinking_delta", text: value.thinking } }, true);
          } else if (value.type === "toolCall" && typeof value.id === "string" && typeof value.name === "string") {
            const args = value.arguments ?? {};
            toolArgs.set(value.id, args);
            this.emit(snapshot, "child_update", { update: { type: "tool_call", toolCallId: value.id, toolName: value.name, args } }, true);
          }
        }
        continue;
      }
      if (child.role === "toolResult" && typeof child.toolCallId === "string" && typeof child.toolName === "string") {
        const args = toolArgs.get(child.toolCallId);
        toolArgs.delete(child.toolCallId);
        this.emit(snapshot, "child_update", { update: { type: "tool_result", toolCallId: child.toolCallId, toolName: child.toolName, args, result: { content: child.content }, isError: child.isError === true } }, true);
      }
    }
    return undefined;
  }

  shutdown(): void {
    for (const record of this.records.values()) {
      if (!record.finished) {
        record.cancelRequested = true;
        record.session.abort();
      }
    }
  }

  private publishTodoBacking(): void {
    const count = [...this.records.values()].filter((record) => record.background && !record.finished).length;
    this.pi.events.emit("pi-grok:todo-backing", { source: "subagent", count });
  }

  private subscribeRecord(record: SubagentRecord): () => void {
    const toolArgs = new Map<string, unknown>();
    return record.session.subscribe((event) => {
      if (event.type === "turn_end") {
        record.turnCount += 1;
        if (record.maxTurns > 0 && record.turnCount >= record.maxTurns && !record.turnLimitReached) {
          record.turnLimitReached = true;
          const summaryPrompt =
            "[SYSTEM] You have reached the maximum number of turns allowed (" +
            String(record.maxTurns) +
            "). Stop all further tool calls immediately. " +
            "Produce your final summary now — return a concise, evidence-based result " +
            "of everything you have gathered so far. Do not make any more tool calls.";
          void record.session.prompt(summaryPrompt, { streamingBehavior: "steer" }).catch(() => undefined);
        }
      }
      if (event.type === "tool_execution_start") {
        record.toolCallCount += 1;
        record.toolsUsed.add(event.toolName);
      }
      if (event.type === "tool_execution_end" && event.isError) record.errorCount += 1;
      if (event.type === "message_end" && event.message.role === "assistant") {
        record.tokensUsed += extractUsage(event.message);
      }
      const update = childUpdate(event, toolArgs);
      if (update) this.emit(record, "child_update", { update });
    });
  }

  private bindAbort(record: SubagentRecord, signal: AbortSignal | undefined): () => void {
    const onAbort = () => {
      record.cancelRequested = true;
      record.session.abort();
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    return () => signal?.removeEventListener("abort", onAbort);
  }

  private finish(record: SubagentRecord, status: "completed" | "failed" | "cancelled", error?: string): void {
    if (record.finished) return;
    record.finished = true;
    this.publishTodoBacking();
    record.terminalStatus = status;
    record.endLeafId = record.session.sessionManager.getLeafId();
    if (error) record.lastError = error;
    record.finalOutputText = lastAssistantText(record.session);
    record.removeAbortListener();
    record.unsubscribe();
    record.doneResolve();
    try {
      this.persistRecord(record, status);
    } catch (persistError) {
      const detail = persistError instanceof Error ? persistError.message : String(persistError);
      record.lastError = `${record.lastError ? `${record.lastError}; ` : ""}State persistence failed: ${detail}`;
    }
    this.emit(record, "finished", {
      status,
      durationMs: Date.now() - record.startedAt,
      turns: record.turnCount,
      toolCalls: record.toolCallCount,
      tokensUsed: record.tokensUsed,
      error: record.lastError,
      output: record.finalOutputText,
    });
    const handler = this.finishHandlers.get(record.id);
    this.finishHandlers.delete(record.id);
    if (handler) void Promise.resolve(handler(record, status, error)).catch(() => undefined);
  }

  async createRecord(
    toolCallId: string,
    params: SpawnParams,
    signal: AbortSignal | undefined,
    ctx: ExtensionContext,
    options: RecordCreateOptions = {},
  ): Promise<SubagentRecord> {
    if (signal?.aborted) throw new Error("Subagent request was cancelled before startup.");
    await this.emit.ready;
    const prompt = requireText(params.prompt, "prompt");
    const description = requireText(params.description, "description");
    const requestedType = params.subagent_type || "general-purpose";
    const definition = selectedDefinition(ctx.cwd, requestedType);
    if (!definition) {
      throw new Error(`Unknown subagent type "${requestedType}". Use /subagents to inspect configured agent definitions.`);
    }
    if (!definition.enabled) {
      throw new Error(`Subagent type \"${definition.name}\" is disabled by its ${definition.scope} Markdown definition.`);
    }
    const profile = profileFor(definition.name, params.capability_mode);
    const configuredModels = definition?.models ?? [];
    const requestedModel = params.model?.trim();
    if (requestedModel && configuredModels.length > 0 && !configuredModels.includes(requestedModel)) {
      throw new Error(
        `Model \"${requestedModel}\" is not enabled for subagent \"${definition?.name ?? requestedType}\". ` +
          `Choose one of: ${configuredModels.join(", ")}.`,
      );
    }
    const modelKey = requestedModel ?? configuredModels[0];
    const availableModels = ctx.modelRegistry.getAvailable?.() ?? ctx.modelRegistry.getAll();
    const selectedModel = modelKey
      ? availableModels.find((candidate) => `${candidate.provider}/${candidate.id}` === modelKey)
      : undefined;
    if (modelKey && !selectedModel) {
      throw new Error(`Configured subagent model \"${modelKey}\" is not currently available in Pi.`);
    }
    const model = selectedModel
      ? ctx.modelRegistry.find(selectedModel.provider, selectedModel.id)
      : ctx.model;
    if (!model) throw new Error("no Pi model is selected");

    const parentSessionFile = ctx.sessionManager.getSessionFile();
    if (!parentSessionFile) throw new Error("parent session persistence is unavailable");

    const agentDir = getAgentDir();
    const settingsManager = SettingsManager.create(ctx.cwd, agentDir);
    const baseSystemPrompt = definition?.systemPrompt || profile.systemPrompt;
    const systemPrompt = options.systemPromptSuffix?.trim()
      ? `${baseSystemPrompt}\n\n${options.systemPromptSuffix.trim()}`
      : baseSystemPrompt;
    // Parent resources, including the built-in multi-agent-proactive skill, do not
    // flow into children. Only an agent definition can explicitly opt a child into
    // extension/skill paths; V1 and V2 both pass through this loader.
    const resourceLoader = new DefaultResourceLoader({
      cwd: ctx.cwd,
      agentDir,
      noExtensions: true,
      noSkills: true,
      additionalExtensionPaths: definition?.extensions ?? [],
      additionalSkillPaths: definition?.skills ?? [],
      noPromptTemplates: true,
      noThemes: true,
      noContextFiles: true,
      systemPromptOverride: () => systemPrompt,
      appendSystemPromptOverride: () => [],
    });
    await resourceLoader.reload();

    const customTools = options.customTools ?? [];
    const businessTools = definition?.tools ?? [...CAPABILITY_TOOLS[profile.capabilityMode]];
    const activeTools = [...new Set([...businessTools, ...customTools.map((tool) => tool.name)])];
    const { session } = await createAgentSession({
      cwd: ctx.cwd,
      agentDir,
      sessionManager: SessionManager.create(ctx.cwd, join(dirname(parentSessionFile), "subagent")),
      settingsManager,
      model,
      tools: activeTools,
      customTools,
      resourceLoader,
    });
    await session.bindExtensions({});
    session.setActiveToolsByName(activeTools);

    const childSessionFile = session.sessionFile;
    if (!childSessionFile) throw new Error("child session persistence is unavailable");
    const parentSessionId = ctx.sessionManager.getSessionId();
    const id = this.allocateRecordId();
    let doneResolve!: () => void;
    const donePromise = new Promise<void>((resolve) => { doneResolve = resolve; });
    const completeRecord: SubagentRecord = {
      id,
      childSessionId: id,
      agentSessionId: session.sessionId,
      childSessionFile,
      startLeafId: session.sessionManager.getLeafId(),
      endLeafId: null,
      stateFile: subagentStateFile(parentSessionFile),
      parentSessionId,
      parentToolCallId: toolCallId,
      prompt,
      description,
      type: profile.type,
      capabilityMode: profile.capabilityMode,
      modelId: model.id,
      background: params.background === true,
      startedAt: Date.now(),
      session,
      turnCount: 0,
      toolCallCount: 0,
      toolsUsed: new Set<string>(),
      errorCount: 0,
      tokensUsed: 0,
      finished: false,
      terminalStatus: null,
      finalOutputText: undefined,
      cancelRequested: false,
      maxTurns: definition?.maxTurns ?? params.max_turns ?? 0,
      turnLimitReached: false,
      donePromise,
      doneResolve,
      removeAbortListener: () => undefined,
      unsubscribe: () => undefined,
    };
    completeRecord.unsubscribe = this.subscribeRecord(completeRecord);

    this.records.set(id, completeRecord);
    if (options.onFinished) this.finishHandlers.set(id, options.onFinished);
    if (completeRecord.background) this.publishTodoBacking();
    this.persistRecord(completeRecord, "running");
    this.emit(completeRecord, "spawned", {
      parentToolCallId: toolCallId,
      description,
      subagentType: completeRecord.type,
      background: completeRecord.background,
      capabilityMode: profile.capabilityMode,
      model: model.id,
      prompt,
    });
    completeRecord.removeAbortListener = this.bindAbort(completeRecord, signal);
    return completeRecord;
  }

  resumeRecord(
    previous: SubagentRecord,
    toolCallId: string,
    prompt: string,
    signal: AbortSignal | undefined,
    onFinished?: RecordFinishHandler,
  ): SubagentRecord {
    if (signal?.aborted) throw new Error("Subagent request was cancelled before reactivation.");
    if (!previous.finished) throw new Error(`subagent ${this.shortSubagentId(previous.id)} is already running`);
    const id = this.allocateRecordId();
    let doneResolve!: () => void;
    const donePromise = new Promise<void>((resolve) => { doneResolve = resolve; });
    const record: SubagentRecord = {
      id,
      childSessionId: id,
      agentSessionId: previous.agentSessionId,
      childSessionFile: previous.childSessionFile,
      startLeafId: previous.session.sessionManager.getLeafId(),
      endLeafId: null,
      stateFile: previous.stateFile,
      parentSessionId: previous.parentSessionId,
      parentToolCallId: toolCallId,
      prompt,
      description: previous.description,
      type: previous.type,
      capabilityMode: previous.capabilityMode,
      modelId: previous.modelId,
      background: previous.background,
      startedAt: Date.now(),
      session: previous.session,
      turnCount: 0,
      toolCallCount: 0,
      toolsUsed: new Set<string>(),
      errorCount: 0,
      tokensUsed: 0,
      finished: false,
      terminalStatus: null,
      finalOutputText: undefined,
      cancelRequested: false,
      maxTurns: previous.maxTurns,
      turnLimitReached: false,
      donePromise,
      doneResolve,
      removeAbortListener: () => undefined,
      unsubscribe: () => undefined,
    };
    record.unsubscribe = this.subscribeRecord(record);
    this.records.set(id, record);
    if (onFinished) this.finishHandlers.set(id, onFinished);
    this.publishTodoBacking();
    this.persistRecord(record, "running");
    this.emit(record, "spawned", {
      parentToolCallId: toolCallId,
      description: record.description,
      subagentType: record.type,
      background: record.background,
      capabilityMode: record.capabilityMode,
      model: record.modelId,
      prompt,
      resumed: true,
      resumedFromSubagentId: previous.id,
    });
    record.removeAbortListener = this.bindAbort(record, signal);
    return record;
  }

  async run(record: SubagentRecord, prompt: string): Promise<string> {
    try {
      await record.session.prompt(prompt);
      if (record.cancelRequested) {
        this.finish(record, "cancelled", "Subagent was cancelled.");
        throw new Error("Subagent was cancelled.");
      }
      const output = lastAssistantText(record.session);
      this.finish(record, "completed");
      return output;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      this.finish(record, record.cancelRequested ? "cancelled" : "failed", message);
      throw error;
    }
  }

  async runCustomMessage(
    record: SubagentRecord,
    message: Parameters<SubagentRecord["session"]["sendCustomMessage"]>[0],
    options: Parameters<SubagentRecord["session"]["sendCustomMessage"]>[1],
  ): Promise<string> {
    try {
      await record.session.sendCustomMessage(message, options);
      if (record.cancelRequested) {
        this.finish(record, "cancelled", "Subagent was cancelled.");
        throw new Error("Subagent was cancelled.");
      }
      const output = lastAssistantText(record.session);
      this.finish(record, "completed");
      return output;
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      this.finish(record, record.cancelRequested ? "cancelled" : "failed", detail);
      throw error;
    }
  }

  scheduleBackgroundTask(record: SubagentRecord, run: () => Promise<void>): "running" | "queued" | "skipped" {
    if (record.finished) return "skipped";
    if (record.cancelRequested) {
      this.finish(record, "cancelled", "Cancelled before background execution started.");
      return "skipped";
    }
    if (this.runningBackground >= MAX_BACKGROUND_CONCURRENCY) {
      this.queuedBackground.push({ record, run });
      return "queued";
    }
    this.runningBackground += 1;
    this.runningBackgroundIds.add(record.id);
    void run()
      .catch(() => undefined)
      .finally(() => {
        this.runningBackgroundIds.delete(record.id);
        this.runningBackground -= 1;
        let next = this.queuedBackground.shift();
        while (next && (next.record.cancelRequested || next.record.finished)) next = this.queuedBackground.shift();
        if (next) this.scheduleBackgroundTask(next.record, next.run);
      });
    return "running";
  }

  backgroundState(record: SubagentRecord): "running" | "queued" | "idle" {
    if (this.runningBackgroundIds.has(record.id)) return "running";
    if (this.queuedBackground.some((entry) => entry.record.id === record.id)) return "queued";
    return "idle";
  }

  scheduleBackground(record: SubagentRecord, prompt: string): void {
    this.scheduleBackgroundTask(record, async () => {
      await this.run(record, prompt);
    });
  }

  recordPostFinishError(record: SubagentRecord, message: string): void {
    record.lastError = `${record.lastError ? `${record.lastError}; ` : ""}${message}`;
    if (!record.terminalStatus) return;
    try {
      this.persistRecord(record, record.terminalStatus);
    } catch (persistError) {
      const detail = persistError instanceof Error ? persistError.message : String(persistError);
      record.lastError += `; State persistence failed: ${detail}`;
    }
  }

  /** Deterministically unique among live records; restart-safe without extra state. */
  private allocateRecordId(): string {
    let id = newSubagentId();
    while (this.records.has(id)) id = newSubagentId();
    return id;
  }

  shortSubagentId(id: string): string {
    return shortSubagentIdFor(id, this.records.keys());
  }

  private matchingSubagents(id: string): SubagentRecord[] {
    const exact = this.records.get(id);
    if (exact) return [exact];
    if (id.length < SHORT_SUBAGENT_ID_LENGTH) return [];
    return [...this.records.values()].filter((record) => record.id.startsWith(id));
  }

  tryResolveSubagent(id: string): SubagentRecord | undefined {
    const matches = this.matchingSubagents(id);
    return matches.length === 1 ? matches[0] : undefined;
  }

  resolveSubagent(id: string): SubagentRecord {
    const matches = this.matchingSubagents(id);
    if (matches.length === 1) return matches[0];
    if (matches.length > 1) throw new Error(`ambiguous subagent ID prefix: ${id}; use more characters`);
    if (id.length < SHORT_SUBAGENT_ID_LENGTH) {
      throw new Error(`subagent ID prefix is too short: ${id}; use at least ${SHORT_SUBAGENT_ID_LENGTH} characters`);
    }
    throw new Error(`unknown subagent: ${id}. Use list_subagents to see available subagents.`);
  }

  runningSubagent(id: string): SubagentRecord {
    const record = this.resolveSubagent(id);
    if (record.finished) {
      throw new Error(`subagent ${this.shortSubagentId(record.id)} has already finished (${this.statusLabel(record)})`);
    }
    return record;
  }

  sendMessage(record: SubagentRecord, message: string, delivery: MessageDelivery): void {
    const streamingBehavior = delivery === "steer" ? "steer" : "followUp";
    void record.session.prompt(message, { streamingBehavior }).catch((error: unknown) => {
      const detail = error instanceof Error ? error.message : String(error);
      record.lastError = `Message delivery failed: ${detail}`;
    });
  }

  cancel(record: SubagentRecord): void {
    if (record.finished) return;
    const queuedIndex = this.queuedBackground.findIndex((entry) => entry.record.id === record.id);
    record.cancelRequested = true;
    if (queuedIndex >= 0) {
      this.queuedBackground.splice(queuedIndex, 1);
      this.finish(record, "cancelled", "Cancelled before background execution started.");
      return;
    }
    record.session.abort();
  }

  discard(record: SubagentRecord, reason: string): void {
    if (record.finished) return;
    this.finishHandlers.delete(record.id);
    record.cancelRequested = true;
    record.session.abort();
    this.finish(record, "cancelled", reason);
  }

  formatDuration(ms: number): string {
    if (ms < 1000) return `${ms}ms`;
    const seconds = Math.floor(ms / 1000);
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    return `${minutes}m${seconds % 60}s`;
  }

  statusLabel(record: SubagentRecord): string {
    if (!record.finished) return "RUNNING";
    return (record.terminalStatus ?? "completed").toUpperCase();
  }

  finalOutput(record: SubagentRecord): string {
    return record.finished && record.finalOutputText !== undefined
      ? record.finalOutputText
      : lastAssistantText(record.session);
  }

  formatSubagentResult(record: SubagentRecord): string {
    const elapsed = this.formatDuration(Date.now() - record.startedAt);
    const status = this.statusLabel(record);
    const shortId = this.shortSubagentId(record.id);
    const header = `[${status}] ${record.description} (${shortId}) — ${elapsed}, ${record.turnCount} turns, ${record.toolCallCount} tool calls`;
    const historyHint = `History: /subagent-history ${shortId}`;
    if (!record.finished) {
      const tools = [...record.toolsUsed].join(", ") || "none yet";
      return `${header}\n${historyHint}\nStatus: still running. Tools used: ${tools}. Tokens: ${record.tokensUsed}.\nUse get_command_or_subagent_output with timeout_ms to wait for completion.`;
    }
    const output = this.finalOutput(record);
    const errorLine = record.lastError ? `\nError: ${record.lastError}` : "";
    if (!output) return `${header}${errorLine}\n${historyHint}\n(Subagent completed without text output.)`;
    const maxOutputChars = 12_000;
    const truncated = output.length > maxOutputChars
      ? `${output.slice(0, maxOutputChars)}\n\n… [truncated ${output.length - maxOutputChars} chars]`
      : output;
    return `${header}${errorLine}\n${historyHint}\n\n${truncated}`;
  }

  waitForRecords(ids: string[], timeoutMs: number, signal?: AbortSignal): Promise<void> {
    const promises = ids.map((id) => this.records.get(id)?.donePromise ?? Promise.resolve());
    const all = Promise.all(promises).then(() => undefined);
    if (timeoutMs <= 0) return all;
    const timeout = new Promise<void>((resolve) => {
      const timer = setTimeout(resolve, Math.min(timeoutMs, MAX_WAIT_MS));
      signal?.addEventListener("abort", () => { clearTimeout(timer); resolve(); }, { once: true });
    });
    return Promise.race([all, timeout]);
  }
}
