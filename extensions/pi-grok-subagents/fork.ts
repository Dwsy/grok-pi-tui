/** Parent-context fork helpers for V1 and V2 subagents (root and child initiators). */

import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { getCurrentSystemMessage, getCurrentTools, uuidv7 } from "@earendil-works/pi-ai";
import {
  CURRENT_SESSION_VERSION,
  SessionManager,
  createCodemodeExtension,
  createMcpExtension,
  createToolSearchExtension,
  type CompactionEntry,
  type ContextEditEntry,
  type Extension,
  type ExtensionAPI,
  type ExtensionContext,
  type ExtensionFactory,
  type InlineExtension,
  type NormalizedBuildSystemPromptOptions,
  type PromptTemplate,
  type SessionContext,
  type SessionEntry,
  type SessionHeader,
  type SessionMessageEntry,
  type SessionProjection,
  type SessionTreeNode,
  type Skill,
} from "@earendil-works/pi-coding-agent";
import { READ_ONLY_SUBAGENT_PROMPT } from "./shared.ts";

export const FORK_CONTEXT_MESSAGE_TYPE = "pi-grok-fork-context/v1";
export const FORK_BRANCH_ANCHOR_TYPE = "pi-grok-fork-branch/v1";
export const FORK_SESSION_INFO_TYPE = "pi-grok-fork-session-info/v1";
export const HOST_GOAL_CUSTOM_TYPE = "pi-grok-goal/v1";

/**
 * Root-owned host UI or orchestration tools that should never leak into a
 * forked child session unless the child caller explicitly injected its own.
 */
export const HOST_ROOT_ONLY_TOOLS = new Set([
  "enter_plan_mode",
  "exit_plan_mode",
  "ask_user_question",
  "goal",
  "workflow",
  "scheduler_create",
  "scheduler_delete",
  "scheduler_list",
  "todo",
  "spawn_team",
]);

/** Commands reserved for host-singleton internal grok-pi extensions. */
export const HOST_BRIDGE_COMMAND_NAMES = new Set([
  "__pi_grok_subagent_replay",
  "__pi_grok_subagent_cancel",
  "__pi_grok_subagent_send",
  "goal",
  "herdr",
  "rollback",
  "recap",
  "plan-file",
]);

const STANDARD_SYSTEM_SECTION_TAGS = new Set([
  "preamble",
  "tools",
  "rules",
  "docs",
  "addendum",
  "project_context",
  "skills",
  "cwd",
  "environment",
  "instructions",
]);

const V1_CONTROL_TOOL_NAMES = new Set([
  "spawn_subagent",
  "send_message_to_subagent",
  "get_command_or_subagent_output",
  "kill_command_or_subagent",
  "list_subagents",
]);

const V2_CONTROL_TOOL_NAMES = new Set([
  "spawn_team_agent",
  "team_send_message",
  "team_followup_task",
  "team_wait",
  "team_list",
  "team_interrupt",
  "team_report",
  "spawn_team",
]);

export type ForkPromptSnapshot = {
  customPrompt?: string;
  forceSystemPrompt?: string;
  appendSystemPrompt?: string[];
  sections: Array<{ tag: string; content: string }>;
  promptGuidelines: string[];
  skills?: Skill[];
  contextFiles?: Array<{ path: string; content: string }>;
  selectedTools?: string[];
};

export type ForkResourceProfile = {
  promptSnapshot?: ForkPromptSnapshot;
  prompts?: PromptTemplate[];
  extensionPaths: string[];
  inlineExtensions?: InlineExtension[];
  noExtensions?: boolean;
  noMcp?: boolean;
  projectTrusted?: boolean;
};

type ReadonlySessionManager = ExtensionContext["sessionManager"];
type TranscriptMessage = SessionContext["messages"][number];

const TEMP_BRIDGE_DIR_RE = /(?:^|\/)pi-grok-[a-z0-9-]+-[A-Za-z0-9._-]+(?:\/|$)/;
const AUTHORED_BRIDGE_EXT_RE = /(?:^|\/)extensions\/pi-grok-[a-z0-9-]+(?:\/|$)/;

/**
 * Returns true when an extension path points to an internal grok-pi host-singleton
 * bridge (materialized temp bundle or authored `extensions/pi-grok-*` source).
 * User, project, npm, and `builtin:*` extensions return false so forked children
 * inherit real plugins while never corrupting root socket/pane/goal/plan state.
 */
export function isHostSingletonBridgeExtension(extensionPath: string): boolean {
  const normalized = extensionPath.replaceAll("\\", "/").trim();
  if (!normalized) return false;
  if (normalized.startsWith("builtin:") || normalized.startsWith("<")) return false;
  return TEMP_BRIDGE_DIR_RE.test(normalized) || AUTHORED_BRIDGE_EXT_RE.test(normalized);
}

/**
 * Defense-in-depth check run inside DefaultResourceLoader `extensionsOverride`:
 * filters out both path-matched grok-pi host bridges and any file extension
 * registering host-singleton tools/commands. Inline factories (`<inline:...>`)
 * injected for the child itself are never filtered here.
 */
export function hasHostSingletonConflict(
  extension: Pick<Extension, "path" | "resolvedPath" | "tools" | "commands">,
): boolean {
  if (extension.path.startsWith("<inline:")) return false;
  if (
    isHostSingletonBridgeExtension(extension.path) ||
    isHostSingletonBridgeExtension(extension.resolvedPath)
  ) {
    return true;
  }
  for (const toolName of extension.tools.keys()) {
    if (HOST_ROOT_ONLY_TOOLS.has(toolName) || V1_CONTROL_TOOL_NAMES.has(toolName) || V2_CONTROL_TOOL_NAMES.has(toolName)) {
      return true;
    }
  }
  for (const commandName of extension.commands.keys()) {
    if (HOST_BRIDGE_COMMAND_NAMES.has(commandName)) {
      return true;
    }
  }
  return false;
}

/** Parse Pi CLI extension flags from process arguments, excluding grok-pi host bridges. */
export function parseProcessExtensionArgs(argv: readonly string[] = process.argv): {
  noExtensions: boolean;
  noMcp: boolean;
  cliExtensionPaths: string[];
  cliSkillPaths: string[];
  cliPromptPaths: string[];
} {
  let noExtensions = false;
  let noMcp = false;
  const cliExtensionPaths: string[] = [];
  const cliSkillPaths: string[] = [];
  const cliPromptPaths: string[] = [];
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--no-extensions" || arg === "-ne") {
      noExtensions = true;
      continue;
    }
    if (arg === "--no-mcp") {
      noMcp = true;
      continue;
    }
    if (arg === "--extension" || arg === "-e") {
      const next = argv[index + 1];
      if (next && !isHostSingletonBridgeExtension(next)) cliExtensionPaths.push(next);
      index += 1;
      continue;
    }
    if (arg.startsWith("--extension=")) {
      const value = arg.slice("--extension=".length);
      if (value && !isHostSingletonBridgeExtension(value)) cliExtensionPaths.push(value);
      continue;
    }
    if (arg === "--skill") {
      const next = argv[index + 1];
      if (next) cliSkillPaths.push(next);
      index += 1;
      continue;
    }
    if (arg.startsWith("--skill=")) {
      const value = arg.slice("--skill=".length);
      if (value) cliSkillPaths.push(value);
      continue;
    }
    if (arg === "--prompt-template") {
      const next = argv[index + 1];
      if (next) cliPromptPaths.push(next);
      index += 1;
      continue;
    }
    if (arg.startsWith("--prompt-template=")) {
      const value = arg.slice("--prompt-template=".length);
      if (value) cliPromptPaths.push(value);
    }
  }
  return {
    noExtensions,
    noMcp,
    cliExtensionPaths: [...new Set(cliExtensionPaths)],
    cliSkillPaths: [...new Set(cliSkillPaths)],
    cliPromptPaths: [...new Set(cliPromptPaths)],
  };
}

/** Discover non-bridge file/builtin extension and prompt paths registered on a live ExtensionAPI. */
export function discoverLoadedExtensionPaths(pi: Partial<ExtensionAPI> | undefined): {
  extensionPaths: string[];
  promptPaths: string[];
} {
  if (!pi) return { extensionPaths: [], promptPaths: [] };
  const extensionPaths = new Set<string>();
  const promptPaths = new Set<string>();
  const addExtension = (candidate: string | undefined) => {
    if (!candidate) return;
    if (candidate.startsWith("builtin:")) {
      if (candidate === "builtin:codemode" || candidate === "builtin:tool-search" || candidate === "builtin:mcp") {
        extensionPaths.add(candidate);
      }
      return;
    }
    if (candidate.startsWith("<")) return;
    if (isHostSingletonBridgeExtension(candidate)) return;
    if (existsSync(candidate)) extensionPaths.add(candidate);
  };
  for (const tool of pi.getAllTools?.() ?? []) {
    addExtension(tool.sourceInfo?.path);
  }
  for (const command of pi.getCommands?.() ?? []) {
    const sourcePath = command.sourceInfo?.path;
    if (command.source === "prompt" && sourcePath && !sourcePath.startsWith("<") && existsSync(sourcePath)) {
      promptPaths.add(sourcePath);
      continue;
    }
    if (command.source === "extension") {
      addExtension(sourcePath);
    }
  }
  return {
    extensionPaths: [...extensionPaths],
    promptPaths: [...promptPaths],
  };
}

/** Build built-in ExtensionFactories needed whenever builtin:codemode / tool-search / mcp are active. */
export function resolveBuiltinExtensionFactories(
  extensionPaths: readonly string[],
  parentActiveTools: readonly string[],
  noMcp: boolean,
): { extraPaths: string[]; builtinFactories: InlineExtension[] } {
  const pathSet = new Set(extensionPaths);
  const activeSet = new Set(parentActiveTools);
  const extraPaths: string[] = [];
  const builtinFactories: InlineExtension[] = [];

  if (pathSet.has("builtin:codemode") || activeSet.has("codemode")) {
    extraPaths.push("builtin:codemode");
    builtinFactories.push({ name: "codemode", factory: createCodemodeExtension(), replaceable: true, builtin: true });
  }
  if (pathSet.has("builtin:tool-search") || activeSet.has("tool_search")) {
    extraPaths.push("builtin:tool-search");
    builtinFactories.push({ name: "tool-search", factory: createToolSearchExtension(), replaceable: true, builtin: true });
  }
  const hasMcp = pathSet.has("builtin:mcp") || parentActiveTools.some((name) => name.startsWith("mcp__"));
  if (!noMcp && hasMcp) {
    extraPaths.push("builtin:mcp");
    builtinFactories.push({ name: "mcp", factory: createMcpExtension(), replaceable: true, builtin: true });
  }

  return { extraPaths: [...new Set(extraPaths)], builtinFactories };
}

/** Clone Pi's mutable before_agent_start prompt options into a point-in-time snapshot. */
export function capturePromptSnapshot(options: NormalizedBuildSystemPromptOptions): ForkPromptSnapshot {
  const appendText = options.appendSystemPrompt.trim();
  const sections = Object.entries(options.sections).map(([tag, content]) => ({ tag, content }));
  return {
    ...(options.customPrompt !== undefined ? { customPrompt: options.customPrompt } : {}),
    ...(options.forceSystemPrompt !== undefined ? { forceSystemPrompt: options.forceSystemPrompt } : {}),
    appendSystemPrompt: appendText ? [options.appendSystemPrompt] : [],
    sections,
    promptGuidelines: [...options.promptGuidelines],
    skills: options.skills.map((skill) => ({ ...skill })),
    contextFiles: options.contextFiles.map((file) => ({ path: file.path, content: file.content })),
    selectedTools: [...options.selectedTools],
  };
}

function unwrapSectionXml(tag: string, value: string): string {
  const prefix = `<${tag}>\n`;
  const suffix = `\n</${tag}>`;
  if (value.startsWith(prefix) && value.endsWith(suffix)) {
    return value.slice(prefix.length, value.length - suffix.length);
  }
  return value;
}

/** Extract custom SystemMessage sections and tool names from a parent transcript projection. */
export function extractTranscriptPromptFallback(messages: readonly TranscriptMessage[]): {
  customSections: Array<{ tag: string; content: string }>;
  activeTools?: string[];
} {
  const systemMessage = getCurrentSystemMessage(messages);
  const customSections: Array<{ tag: string; content: string }> = [];
  if (systemMessage?.sections) {
    for (const [tag, content] of Object.entries(systemMessage.sections)) {
      if (!STANDARD_SYSTEM_SECTION_TAGS.has(tag) && typeof content === "string") {
        customSections.push({ tag, content: unwrapSectionXml(tag, content) });
      }
    }
  }
  const transcriptTools = getCurrentTools(messages).map((tool) => tool.name);
  return {
    customSections,
    ...(transcriptTools.length > 0 ? { activeTools: transcriptTools } : {}),
  };
}

/** Inline ExtensionFactory attached to children so nested child-initiated forks capture prompt state. */
export function createPromptCaptureExtensionFactory(
  onCapture: (snapshot: ForkPromptSnapshot) => void,
): ExtensionFactory {
  return (pi) => {
    let lastOptions: NormalizedBuildSystemPromptOptions | undefined;
    pi.on("before_agent_start", (event) => {
      lastOptions = event.systemPromptOptions;
      onCapture(capturePromptSnapshot(event.systemPromptOptions));
    });
    pi.on("agent_start", () => {
      if (lastOptions) onCapture(capturePromptSnapshot(lastOptions));
    });
  };
}

/** Build the single non-displayed context directive injected on turn 1 of a forked child. */
export function composeForkedContextDirective(input: {
  roleName?: string;
  rolePrompt?: string;
  systemPromptSuffix?: string;
  readOnly: boolean;
}): string {
  const parts = [
    "<forked_subagent_context>",
    "You are running inside an isolated subagent session forked from your parent agent's live context.",
    "Conversation messages and tool results above the fork boundary belong to the parent branch. Do not repeat completed parent work; if earlier context mentions a prior agent identity or /root path, this forked branch's own identity, role, and task below supersede it.",
  ];
  if (input.roleName && input.rolePrompt?.trim()) {
    parts.push(`Role (${input.roleName}):\n${input.rolePrompt.trim()}`);
  }
  if (input.systemPromptSuffix?.trim()) {
    parts.push(input.systemPromptSuffix.trim());
  }
  if (input.readOnly) {
    parts.push(READ_ONLY_SUBAGENT_PROMPT);
  }
  parts.push("</forked_subagent_context>");
  return parts.join("\n\n");
}

/**
 * Inline ExtensionFactory for a forked child:
 * - Replays parent's custom sections (such as `reply_style`), `promptGuidelines`,
 *   and `forceSystemPrompt` on `before_agent_start` so `diffSystemPromptSections`
 *   preserves the parent SystemMessage structure and KV cache prefix.
 * - Injects `<forked_subagent_context>` once as a non-displayed custom message
 *   alongside the child's first prompt turn.
 */
export function createForkContextExtensionFactory(
  snapshot: ForkPromptSnapshot | undefined,
  directive: string,
): ExtensionFactory {
  let directiveInjected = false;
  return (pi) => {
    pi.on("before_agent_start", (event) => {
      if (snapshot) {
        if (snapshot.forceSystemPrompt !== undefined) {
          event.systemPromptOptions.forceSystemPrompt = snapshot.forceSystemPrompt;
        }
        const sections = (event.systemPromptOptions.sections ??= {});
        for (const section of snapshot.sections) {
          if (sections[section.tag] === undefined) {
            sections[section.tag] = section.content;
          }
        }
        if (snapshot.promptGuidelines.length > 0) {
          const merged = new Set([
            ...(event.systemPromptOptions.promptGuidelines ?? []),
            ...snapshot.promptGuidelines,
          ]);
          event.systemPromptOptions.promptGuidelines = [...merged];
        }
      }
      if (!directiveInjected) {
        directiveInjected = true;
        return {
          message: {
            customType: FORK_CONTEXT_MESSAGE_TYPE,
            content: directive,
            display: false,
          },
        };
      }
      return undefined;
    });
  };
}

function generateUniqueEntryId(usedIds: Set<string>): string {
  for (let attempt = 0; attempt < 16; attempt += 1) {
    const candidate = randomBytes(4).toString("hex");
    if (!usedIds.has(candidate)) return candidate;
  }
  return uuidv7();
}

function shouldSkipForkEntry(entry: SessionEntry): boolean {
  if (entry.type === "label") return true;
  if (entry.type === "custom" && entry.customType === HOST_GOAL_CUSTOM_TYPE) return true;
  if (entry.type === "custom_message" && entry.customType === FORK_CONTEXT_MESSAGE_TYPE) return true;
  return false;
}

function collectOpenToolCalls(entries: readonly SessionEntry[]): Array<{ id: string; name: string }> {
  // Detect unresolved tool calls only if the branch currently ends inside an
  // assistant -> toolResult sequence (e.g. parallel tool calls in flight).
  const openToolCalls = new Map<string, { id: string; name: string }>();
  for (const entry of entries) {
    if (
      entry.type === "compaction" ||
      entry.type === "branch_summary" ||
      entry.type === "custom_message"
    ) {
      openToolCalls.clear();
      continue;
    }
    if (entry.type !== "message") continue;
    const message = entry.message as {
      role?: unknown;
      content?: unknown;
      toolCallId?: unknown;
    };
    if (message.role === "assistant") {
      openToolCalls.clear();
      if (Array.isArray(message.content)) {
        for (const block of message.content) {
          if (
            typeof block === "object" &&
            block !== null &&
            (block as { type?: unknown }).type === "toolCall" &&
            typeof (block as { id?: unknown }).id === "string" &&
            typeof (block as { name?: unknown }).name === "string"
          ) {
            const call = block as { id: string; name: string };
            openToolCalls.set(call.id, { id: call.id, name: call.name });
          }
        }
      }
      continue;
    }
    if (message.role === "toolResult") {
      if (typeof message.toolCallId === "string") {
        openToolCalls.delete(message.toolCallId);
      }
      continue;
    }
    if (
      message.role === "user" ||
      message.role === "bashExecution" ||
      message.role === "branchSummary" ||
      message.role === "compactionSummary"
    ) {
      openToolCalls.clear();
    }
  }
  return [...openToolCalls.values()];
}

function syntheticToolResultMessage(pending: { id: string; name: string }, spawningToolCallId: string | undefined, nowMs: number) {
  const note =
    pending.id === spawningToolCallId
      ? "Forked child session branched from this tool call. Proceed with the delegated child task."
      : "Parallel tool call belongs to the parent turn at the fork boundary; proceed with the delegated child task.";
  return {
    role: "toolResult" as const,
    toolCallId: pending.id,
    toolName: pending.name,
    content: [{ type: "text" as const, text: note }],
    isError: false,
    timestamp: nowMs,
  };
}

function formatForkBranchLabel(agentPath?: string, description?: string, fallbackId?: string): string | undefined {
  const cleanDesc = description?.replace(/[\r\n\s]+/g, " ").trim();
  if (agentPath && cleanDesc) return `fork ${agentPath}: ${cleanDesc}`;
  if (agentPath) return `fork ${agentPath}`;
  if (cleanDesc) return `fork: ${cleanDesc}`;
  return fallbackId ? `fork: ${fallbackId}` : undefined;
}

/**
 * Isolated-cursor SessionManager view over a parent SessionManager + single JSONL file.
 *
 * Every synchronous read and append swaps the backing SessionManager's leaf
 * cursor to this branch's `cursorLeafId`, executes synchronously inside Node's
 * single-threaded turn, and restores the backing leaf cursor in `finally`.
 * Root and multiple concurrent forked subagents therefore write independent
 * sibling/nested branches directly into the same `.jsonl` file and Pi `/tree`
 * without ever corrupting each other's `leafId`.
 */
export class SharedBranchSessionManager {
  readonly backingSessionManager: SessionManager;
  private cursorLeafId: string | null;
  private readonly branchSessionId: string;
  private readonly branchLabel?: string;
  private labeledFirstUserEntry = false;
  private localSessionName?: string;

  constructor(
    backingSessionManager: SessionManager,
    initialLeafId: string | null,
    branchSessionId: string,
    branchLabel?: string,
  ) {
    this.backingSessionManager = backingSessionManager;
    this.cursorLeafId = initialLeafId;
    this.branchSessionId = branchSessionId;
    this.branchLabel = branchLabel;
  }

  private setBackingLeaf(leafId: string | null): void {
    if (leafId === null) this.backingSessionManager.resetLeaf();
    else this.backingSessionManager.branch(leafId);
  }

  private withReadCursor<T>(fn: () => T): T {
    const savedLeaf = this.backingSessionManager.getLeafId();
    this.setBackingLeaf(this.cursorLeafId);
    try {
      return fn();
    } finally {
      this.setBackingLeaf(savedLeaf);
    }
  }

  private mutateWithCursor<T>(fn: () => T): T {
    const savedLeaf = this.backingSessionManager.getLeafId();
    this.setBackingLeaf(this.cursorLeafId);
    try {
      const result = fn();
      this.cursorLeafId = this.backingSessionManager.getLeafId();
      return result;
    } finally {
      this.setBackingLeaf(savedLeaf);
    }
  }

  isPersisted(): boolean {
    return this.backingSessionManager.isPersisted();
  }

  getCwd(): string {
    return this.backingSessionManager.getCwd();
  }

  getSessionDir(): string {
    return this.backingSessionManager.getSessionDir();
  }

  usesDefaultSessionDir(): boolean {
    return this.backingSessionManager.usesDefaultSessionDir();
  }

  getSessionId(): string {
    return this.branchSessionId;
  }

  getSessionFile(): string | undefined {
    return this.backingSessionManager.getSessionFile();
  }

  getHeader(): SessionHeader | null {
    return this.backingSessionManager.getHeader();
  }

  getEntryCount(): number {
    return this.backingSessionManager.getEntryCount();
  }

  getEntries(): SessionEntry[] {
    return this.backingSessionManager.getEntries();
  }

  getTree(): SessionTreeNode[] {
    return this.backingSessionManager.getTree();
  }

  getLeafId(): string | null {
    return this.cursorLeafId;
  }

  getLeafEntry(): SessionEntry | undefined {
    return this.cursorLeafId ? this.backingSessionManager.getEntry(this.cursorLeafId) : undefined;
  }

  getEntry(id: string): SessionEntry | undefined {
    return this.backingSessionManager.getEntry(id);
  }

  getChildren(parentId: string): SessionEntry[] {
    return this.backingSessionManager.getChildren(parentId);
  }

  getLabel(id: string): string | undefined {
    return this.backingSessionManager.getLabel(id);
  }

  getBranch(fromId?: string): SessionEntry[] {
    if (fromId !== undefined) return this.backingSessionManager.getBranch(fromId);
    return this.cursorLeafId ? this.backingSessionManager.getBranch(this.cursorLeafId) : [];
  }

  buildContextEntries(): SessionEntry[] {
    return this.withReadCursor(() => this.backingSessionManager.buildContextEntries());
  }

  buildSessionProjection(): SessionProjection {
    return this.withReadCursor(() => this.backingSessionManager.buildSessionProjection());
  }

  buildSessionContext(): SessionContext {
    return this.withReadCursor(() => this.backingSessionManager.buildSessionContext());
  }

  getSessionName(): string | undefined {
    return this.localSessionName ?? this.backingSessionManager.getSessionName();
  }

  branch(branchFromId: string): void {
    if (!this.backingSessionManager.getEntry(branchFromId)) {
      throw new Error(`Entry ${branchFromId} not found`);
    }
    this.cursorLeafId = branchFromId;
  }

  resetLeaf(): void {
    this.cursorLeafId = null;
  }

  branchWithSummary(
    branchFromId: string | null,
    summary: string,
    details?: unknown,
    fromHook?: boolean,
    usage?: CompactionEntry["usage"],
  ): string {
    return this.mutateWithCursor(() =>
      this.backingSessionManager.branchWithSummary(branchFromId, summary, details, fromHook, usage),
    );
  }

  createBranchedSession(leafId: string): string | undefined {
    return this.backingSessionManager.createBranchedSession(leafId);
  }

  appendMessage(message: Parameters<SessionManager["appendMessage"]>[0]): string {
    const entryId = this.mutateWithCursor(() => this.backingSessionManager.appendMessage(message));
    if (!this.labeledFirstUserEntry && (message as { role?: unknown }).role === "user" && this.branchLabel) {
      this.labeledFirstUserEntry = true;
      this.mutateWithCursor(() => this.backingSessionManager.appendLabelChange(entryId, this.branchLabel));
    }
    return entryId;
  }

  appendThinkingLevelChange(thinkingLevel: string): string {
    return this.mutateWithCursor(() => this.backingSessionManager.appendThinkingLevelChange(thinkingLevel));
  }

  appendModelChange(provider: string, modelId: string): string {
    return this.mutateWithCursor(() => this.backingSessionManager.appendModelChange(provider, modelId));
  }

  appendUsage(
    kind: string,
    provider: string,
    model: string,
    usage: Parameters<SessionManager["appendUsage"]>[3],
    note?: string,
  ) {
    return this.mutateWithCursor(() => this.backingSessionManager.appendUsage(kind, provider, model, usage, note));
  }

  appendCompaction<T = unknown>(
    summary: string,
    firstKeptEntryId: string | null,
    tokensBefore: number,
    details?: T,
    fromHook?: boolean,
    usage?: Parameters<SessionManager["appendCompaction"]>[5],
  ): string {
    return this.mutateWithCursor(() =>
      this.backingSessionManager.appendCompaction(summary, firstKeptEntryId, tokensBefore, details, fromHook, usage),
    );
  }

  appendCustomEntry(customType: string, data?: unknown): string {
    return this.mutateWithCursor(() => this.backingSessionManager.appendCustomEntry(customType, data));
  }

  appendSessionInfo(name: string): string {
    this.localSessionName = name.replace(/[\r\n]+/g, " ").trim() || undefined;
    // Keep child session naming local instead of overwriting the root session title.
    return this.appendCustomEntry(FORK_SESSION_INFO_TYPE, { name: this.localSessionName ?? "" });
  }

  appendCustomMessageEntry<T = unknown>(
    customType: string,
    content: Parameters<SessionManager["appendCustomMessageEntry"]>[1],
    display: boolean,
    details?: T,
  ): string {
    return this.mutateWithCursor(() =>
      this.backingSessionManager.appendCustomMessageEntry(customType, content, display, details),
    );
  }

  appendContextEdit(targetId: string, replacement: ContextEditEntry["replacement"]): string {
    return this.mutateWithCursor(() => this.backingSessionManager.appendContextEdit(targetId, replacement));
  }

  appendLabelChange(targetId: string, label: string | undefined): string {
    return this.mutateWithCursor(() => this.backingSessionManager.appendLabelChange(targetId, label));
  }
}

function createStandaloneForkedSessionManager(options: {
  parentSessionManager: ReadonlySessionManager;
  parentSessionFile: string;
  subagentDir: string;
  cwd: string;
  spawningToolCallId?: string;
}): SessionManager {
  const sourceBranch = options.parentSessionManager.getBranch();
  const keptEntries: SessionEntry[] = [];
  const replacementBySkippedId = new Map<string, string>();
  const pendingSkippedIds: string[] = [];
  const keptIds = new Set<string>();
  let pathParentId: string | null = null;

  for (const entry of sourceBranch) {
    if (shouldSkipForkEntry(entry)) {
      pendingSkippedIds.push(entry.id);
      continue;
    }
    if (entry.type === "context_edit" && !keptIds.has(entry.targetId)) {
      pendingSkippedIds.push(entry.id);
      continue;
    }
    for (const skippedId of pendingSkippedIds) {
      replacementBySkippedId.set(skippedId, entry.id);
    }
    pendingSkippedIds.length = 0;

    const cloned: SessionEntry =
      entry.type === "compaction"
        ? {
            ...entry,
            parentId: pathParentId,
            firstKeptEntryId:
              entry.firstKeptEntryId === entry.id
                ? entry.id
                : (replacementBySkippedId.get(entry.firstKeptEntryId) ?? entry.firstKeptEntryId),
          }
        : { ...entry, parentId: pathParentId };
    keptEntries.push(cloned);
    keptIds.add(cloned.id);
    pathParentId = cloned.id;
  }

  const nowMs = Date.now();
  const nowIso = new Date(nowMs).toISOString();
  for (const pending of collectOpenToolCalls(keptEntries)) {
    const synthId = generateUniqueEntryId(keptIds);
    keptIds.add(synthId);
    const synthEntry: SessionMessageEntry = {
      type: "message",
      id: synthId,
      parentId: pathParentId,
      timestamp: nowIso,
      message: syntheticToolResultMessage(pending, options.spawningToolCallId, nowMs),
    };
    keptEntries.push(synthEntry);
    pathParentId = synthId;
  }

  const newSessionId = uuidv7();
  const fileTimestamp = nowIso.replace(/[:.]/g, "-");
  mkdirSync(options.subagentDir, { recursive: true });
  const newSessionFile = join(options.subagentDir, `${fileTimestamp}_${newSessionId}.jsonl`);
  const header: SessionHeader = {
    type: "session",
    version: CURRENT_SESSION_VERSION,
    id: newSessionId,
    timestamp: nowIso,
    cwd: options.cwd,
    parentSession: options.parentSessionFile,
  };
  const lines = [JSON.stringify(header), ...keptEntries.map((entry) => JSON.stringify(entry))].join("\n") + "\n";
  writeFileSync(newSessionFile, lines, { encoding: "utf8", mode: 0o600 });
  return SessionManager.open(newSessionFile, options.subagentDir, options.cwd);
}

function unwrapMutableSessionManager(candidate: ReadonlySessionManager): SessionManager | undefined {
  if (candidate instanceof SharedBranchSessionManager) {
    return candidate.backingSessionManager;
  }
  const maybe = candidate as Partial<SessionManager>;
  if (
    typeof maybe.branch === "function" &&
    typeof maybe.resetLeaf === "function" &&
    typeof maybe.appendMessage === "function" &&
    typeof maybe.appendCustomEntry === "function" &&
    typeof maybe.appendContextEdit === "function" &&
    typeof maybe.appendLabelChange === "function"
  ) {
    return candidate as unknown as SessionManager;
  }
  return undefined;
}

/**
 * Create a child SessionManager forked from the parent's active branch up to
 * the fork point:
 * - For real Pi sessions, shares the parent's live SessionManager and `.jsonl`
 *   file via `SharedBranchSessionManager` so forked children and grandchildren
 *   become real navigable branches inside the same session tree (`/tree`).
 * - Falls back to creating an isolated copy only when the caller passes a
 *   partial non-mutable mock `ReadonlySessionManager`.
 */
export function createForkedChildSessionManager(options: {
  parentSessionManager: ReadonlySessionManager;
  parentSessionFile: string;
  subagentDir: string;
  cwd: string;
  spawningToolCallId?: string;
  description?: string;
  agentPath?: string;
}): SessionManager {
  const backing = unwrapMutableSessionManager(options.parentSessionManager);
  if (!backing) {
    return createStandaloneForkedSessionManager(options);
  }

  const forkOriginLeafId = options.parentSessionManager.getLeafId();
  const sourceBranch = options.parentSessionManager.getBranch();
  const branchSuffix = randomBytes(4).toString("hex");
  const branchSessionId = `${backing.getSessionId()}-fork-${branchSuffix}`;
  const branchLabel = formatForkBranchLabel(options.agentPath, options.description, branchSuffix);
  const shared = new SharedBranchSessionManager(backing, forkOriginLeafId, branchSessionId, branchLabel);

  // Anchor each forked child subtree explicitly under the shared fork-point entry
  // before appending setup entries so parallel siblings get unique `startLeafId`s
  // and tree history walkers can stop cleanly at nested fork boundaries.
  shared.appendCustomEntry(FORK_BRANCH_ANCHOR_TYPE, {
    branchSessionId,
    forkOriginLeafId,
    ...(options.description ? { description: options.description } : {}),
    ...(options.agentPath ? { agentPath: options.agentPath } : {}),
    ...(options.spawningToolCallId ? { spawningToolCallId: options.spawningToolCallId } : {}),
  });

  const nowMs = Date.now();
  for (const pending of collectOpenToolCalls(sourceBranch)) {
    shared.appendMessage(syntheticToolResultMessage(pending, options.spawningToolCallId, nowMs));
  }

  // Hide any ancestor FORK_CONTEXT_MESSAGE_TYPE only along this grandchild subtree
  // (leaving the parent's branch untouched for reactivations/inspections).
  const activeContextEntries = shared.buildContextEntries();
  const omittedIds = new Set<string>();
  for (const entry of activeContextEntries) {
    if (entry.type === "context_edit" && entry.replacement === null) {
      omittedIds.add(entry.targetId);
    }
  }
  for (const entry of activeContextEntries) {
    if (entry.type === "custom_message" && entry.customType === FORK_CONTEXT_MESSAGE_TYPE && !omittedIds.has(entry.id)) {
      shared.appendContextEdit(entry.id, null);
    }
  }

  return shared as unknown as SessionManager;
}

/**
 * Resolve the child session's active tool list in fork mode:
 * - Preserves parent active tool order first, then appends extension-added and
 *   child custom control tools.
 * - Excludes host-root-only tools and any tool not registered in the child.
 * - Drops `edit` and `write` when `readOnly` is true while keeping `bash`/`read`
 *   if available.
 */
export function resolveForkActiveTools(options: {
  parentActiveTools: readonly string[];
  availableTools: readonly string[];
  customToolNames: readonly string[];
  addedExtensionTools: readonly string[];
  readOnly: boolean;
}): string[] {
  const available = new Set(options.availableTools);
  const customSet = new Set(options.customToolNames);
  const ordered = [
    ...options.parentActiveTools,
    ...options.addedExtensionTools,
    ...options.customToolNames,
  ];
  const result: string[] = [];
  const seen = new Set<string>();
  for (const name of ordered) {
    if (seen.has(name)) continue;
    if (!available.has(name)) continue;
    if (HOST_ROOT_ONLY_TOOLS.has(name) && !customSet.has(name)) continue;
    if (options.readOnly && (name === "edit" || name === "write")) continue;
    seen.add(name);
    result.push(name);
  }
  return result;
}
