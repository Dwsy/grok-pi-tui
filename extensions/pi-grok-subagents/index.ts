import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join } from "node:path";
import { Type } from "typebox";
import {
  createAgentSession,
  DefaultResourceLoader,
  getAgentDir,
  parseFrontmatter,
  SessionManager,
  SettingsManager,
  type AgentSession,
  type AgentSessionEvent,
  type ExtensionCommandContext,
  type ExtensionAPI,
  type ExtensionContext,
} from "@earendil-works/pi-coding-agent";

const BRIDGE_TYPE = "pi-grok-subagent/v1";
const STATE_ENTRY_TYPE = "pi-grok-subagent-state/v1";
const PROGRESS_INTERVAL_MS = 2_000;
const MAX_BACKGROUND_CONCURRENCY = 4;
const MAX_WAIT_MS = 600_000; // 10 minutes cap for blocking waits
const POLL_INTERVAL_MS = 500;
const MAX_AGENT_MODELS = 3;
const SHORT_SUBAGENT_ID_LENGTH = 8;
const MULTI_SELECT_TITLE_PREFIX = "__pi_grok_multi_select_v1__:";
const RESOURCE_PICKER_TITLE_PREFIX = "__pi_grok_resource_picker_v1__:";

const BUILTIN_TOOL_NAMES = ["read", "bash", "edit", "write", "grep", "find", "ls"] as const;
const BUILTIN_TOOL_SET = new Set<string>(BUILTIN_TOOL_NAMES);

const CAPABILITY_TOOLS = {
  "read-only": ["read", "grep", "find", "ls"],
  "read-write": ["read", "grep", "find", "ls", "edit", "write"],
  execute: ["read", "bash", "grep", "find", "ls"],
  all: [...BUILTIN_TOOL_NAMES],
} as const;

type CapabilityMode = keyof typeof CAPABILITY_TOOLS;

const AGENT_PROFILES: Record<string, { capabilityMode: CapabilityMode; systemPrompt: string }> = {
  "general-purpose": {
    capabilityMode: "all",
    systemPrompt: "You are a focused coding subagent. Complete only the delegated task and return a concise evidence-based result.",
  },
  explore: {
    capabilityMode: "execute",
    systemPrompt: "You are a read-only exploration subagent. Inspect the codebase, run safe diagnostic commands, and report evidence without editing files.",
  },
  plan: {
    capabilityMode: "execute",
    systemPrompt: "You are a planning subagent. Inspect the codebase and return an implementation plan with risks and verification steps. Do not edit files.",
  },
};

type AgentDefinitionScope = "builtin" | "global" | "project";

type AgentDefinition = {
  name: string;
  scope: AgentDefinitionScope;
  builtin?: boolean;
  enabled: boolean;
  description: string;
  systemPrompt: string;
  tools?: string[];
  models?: string[];
  extensions?: string[];
  skills?: string[];
  maxTurns?: number;
};

type CatalogEntry = {
  id: string;
  label: string;
  description?: string;
};

type InjectedExtensionCatalogEntry = {
  path: string;
  label: string;
  description?: string;
};

type ResourcePickerExtra = {
  path: string;
  label: string;
  type: "extensions" | "skills";
};

function productProjectDir(cwd: string): string {
  const configured = process.env.GROK_PROJECT_DIR?.trim() || ".grok-pi";
  return isAbsolute(configured) ? configured : join(cwd, configured);
}

function productGlobalDir(): string {
  return process.env.GROK_HOME?.trim() || join(homedir(), ".grok-pi");
}

function definitionDir(cwd: string, scope: AgentDefinitionScope): string {
  if (scope === "builtin") throw new Error("Built-in subagents do not have a definition directory.");
  return scope === "project" ? join(productProjectDir(cwd), "agents") : join(productGlobalDir(), "agents");
}

function definitionPath(cwd: string, scope: AgentDefinitionScope, name: string): string {
  return join(definitionDir(cwd, scope), `${name}.md`);
}

function definitionName(value: string): string | undefined {
  const name = value.trim();
  return /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(name) ? name : undefined;
}

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function optionalList(value: unknown): string[] | undefined {
  if (value === undefined || value === null || value === false || value === "none") return undefined;
  const raw = Array.isArray(value)
    ? value.filter((entry): entry is string => typeof entry === "string")
    : typeof value === "string"
      ? value.split(",")
      : [];
  const values = [...new Set(raw.map((entry) => entry.trim()).filter(Boolean))];
  return Array.isArray(value) ? values : values.length > 0 ? values : undefined;
}

function optionalMaxTurns(value: unknown): number | undefined {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : undefined;
}

function loadDefinitionsFromDir(
  cwd: string,
  scope: AgentDefinitionScope,
  definitions: Map<string, AgentDefinition>,
): void {
  const dir = definitionDir(cwd, scope);
  if (!existsSync(dir)) return;
  let files: string[];
  try {
    files = readdirSync(dir).filter((file) => file.endsWith(".md")).sort();
  } catch {
    return;
  }
  for (const file of files) {
    const name = definitionName(basename(file, ".md"));
    if (!name) continue;
    try {
      const { frontmatter, body } = parseFrontmatter<Record<string, unknown>>(readFileSync(join(dir, file), "utf8"));
      const models = optionalList(frontmatter.models ?? frontmatter.model)?.slice(0, MAX_AGENT_MODELS);
      definitions.set(name.toLowerCase(), {
        name,
        scope,
        builtin: Object.hasOwn(AGENT_PROFILES, name.toLowerCase()),
        enabled: frontmatter.enabled !== false,
        description: optionalString(frontmatter.description) ?? name,
        systemPrompt: body.trim(),
        tools: optionalList(frontmatter.tools),
        models,
        extensions: optionalList(frontmatter.extensions),
        skills: optionalList(frontmatter.skills),
        maxTurns: optionalMaxTurns(frontmatter.max_turns),
      });
    } catch {
      // A malformed local definition must not stop unrelated agents from loading.
    }
  }
}

function loadAgentDefinitions(cwd: string): Map<string, AgentDefinition> {
  const definitions = new Map<string, AgentDefinition>();
  for (const [name, profile] of Object.entries(AGENT_PROFILES)) {
    definitions.set(name, {
      name,
      scope: "builtin",
      builtin: true,
      enabled: true,
      description: `Built-in ${name} subagent`,
      systemPrompt: profile.systemPrompt,
    });
  }
  loadDefinitionsFromDir(cwd, "global", definitions);
  // A project definition, including `enabled: false`, deliberately overrides
  // the same global name. This is the project-level global-off switch.
  loadDefinitionsFromDir(cwd, "project", definitions);
  return definitions;
}

function yamlList(values: string[] | undefined, includeEmpty = false): string | undefined {
  return values && (includeEmpty || values.length > 0) ? JSON.stringify(values) : undefined;
}

function serializeDefinition(definition: AgentDefinition): string {
  const fields = [
    `description: ${JSON.stringify(definition.description)}`,
    `enabled: ${definition.enabled ? "true" : "false"}`,
    yamlList(definition.tools, true) && `tools: ${yamlList(definition.tools, true)}`,
    yamlList(definition.models) && `models: ${yamlList(definition.models)}`,
    yamlList(definition.extensions) && `extensions: ${yamlList(definition.extensions)}`,
    yamlList(definition.skills) && `skills: ${yamlList(definition.skills)}`,
    definition.maxTurns !== undefined && `max_turns: ${definition.maxTurns}`,
  ].filter((field): field is string => Boolean(field));
  return `---\n${fields.join("\n")}\n---\n\n${definition.systemPrompt.trim()}\n`;
}

function saveDefinition(cwd: string, definition: AgentDefinition): void {
  if (definition.scope === "builtin") throw new Error("Choose project or global scope before saving a built-in subagent.");
  const dir = definitionDir(cwd, definition.scope);
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const destination = definitionPath(cwd, definition.scope, definition.name);
  const temporary = `${destination}.${process.pid}.${randomUUID()}.tmp`;
  writeFileSync(temporary, serializeDefinition(definition), { encoding: "utf8", mode: 0o600 });
  renameSync(temporary, destination);
}

function deleteDefinition(cwd: string, definition: AgentDefinition): void {
  if (definition.scope === "builtin") return;
  const path = definitionPath(cwd, definition.scope, definition.name);
  if (existsSync(path)) unlinkSync(path);
}

function cloneDefinition(definition: AgentDefinition): AgentDefinition {
  return {
    ...definition,
    tools: definition.tools?.slice(),
    models: definition.models?.slice(),
    extensions: definition.extensions?.slice(),
    skills: definition.skills?.slice(),
  };
}

function profileFor(type: string, capabilityMode?: string): { type: string; capabilityMode: CapabilityMode; systemPrompt: string } {
  return resolveProfile(type, capabilityMode);
}

function selectedDefinition(cwd: string, type: string): AgentDefinition | undefined {
  return loadAgentDefinitions(cwd).get(type.trim().toLowerCase());
}

function catalogLabel(entry: CatalogEntry, selected: Set<string>): string {
  return `${selected.has(entry.id) ? "☑" : "☐"} ${entry.label}`;
}

async function selectToggles(
  ctx: ExtensionCommandContext,
  title: string,
  entries: CatalogEntry[],
  selected: Set<string>,
  maxSelections?: number,
): Promise<Set<string> | undefined> {
  if (entries.length === 0) {
    ctx.ui.notify(`No ${title.toLowerCase()} are currently available.`, "warning");
    return selected;
  }
  const labels = new Map<string, string>();
  for (const entry of entries) labels.set(catalogLabel(entry, selected), entry.id);
  const metadata = JSON.stringify({ title: `${title}: choose items to toggle (☑ selected)`, maxSelections });
  const raw = await ctx.ui.select(`${MULTI_SELECT_TITLE_PREFIX}${metadata}`, [...labels.keys()]);
  if (raw === undefined) return undefined;
  let toggledLabels: unknown;
  try {
    toggledLabels = JSON.parse(raw);
  } catch {
    return selected;
  }
  if (!Array.isArray(toggledLabels) || !toggledLabels.every((label) => typeof label === "string")) return selected;
  const next = new Set(selected);
  for (const label of toggledLabels) {
    const id = labels.get(label);
    if (!id) continue;
    if (next.has(id)) next.delete(id);
    else next.add(id);
  }
  if (maxSelections !== undefined && next.size > maxSelections) {
    ctx.ui.notify(`Select at most ${maxSelections} models.`, "warning");
    return selected;
  }
  return next;
}

function toolCatalog(pi: ExtensionAPI): { builtin: CatalogEntry[]; plugin: CatalogEntry[] } {
  const builtin: CatalogEntry[] = [];
  const plugin: CatalogEntry[] = [];
  for (const tool of pi.getAllTools()) {
    const entry = { id: tool.name, label: tool.name, description: tool.description };
    if (BUILTIN_TOOL_SET.has(tool.name)) builtin.push(entry);
    else plugin.push(entry);
  }
  const sort = (left: CatalogEntry, right: CatalogEntry) => left.label.localeCompare(right.label);
  return { builtin: builtin.sort(sort), plugin: plugin.sort(sort) };
}

function extensionCatalog(pi: ExtensionAPI): CatalogEntry[] {
  const byPath = new Map<string, CatalogEntry>();
  const injected = process.env.PI_GROK_SUBAGENT_EXTENSION_CATALOG;
  if (injected) {
    try {
      const parsed: unknown = JSON.parse(injected);
      if (Array.isArray(parsed)) {
        for (const entry of parsed) {
          if (
            typeof entry === "object" &&
            entry !== null &&
            typeof (entry as InjectedExtensionCatalogEntry).path === "string" &&
            typeof (entry as InjectedExtensionCatalogEntry).label === "string"
          ) {
            const catalogEntry = entry as InjectedExtensionCatalogEntry;
            byPath.set(catalogEntry.path, {
              id: catalogEntry.path,
              label: catalogEntry.label,
              description: catalogEntry.description ?? "Pi extension available in this Grok-Pi session",
            });
          }
        }
      }
    } catch {
      // A malformed host catalog must not prevent normal Pi tool discovery.
    }
  }
  for (const tool of pi.getAllTools()) {
    if (BUILTIN_TOOL_SET.has(tool.name)) continue;
    const path = tool.sourceInfo.path;
    if (!path || byPath.has(path)) continue;
    byPath.set(path, {
      id: path,
      label: basename(path),
      description: `${tool.sourceInfo.scope} extension; provides ${tool.name}`,
    });
  }
  return [...byPath.values()].sort((left, right) => left.label.localeCompare(right.label));
}

async function selectResources(
  ctx: ExtensionCommandContext,
  title: string,
  resourceType: "extensions" | "skills",
  current: Set<string>,
  extraResources: ResourcePickerExtra[] = [],
): Promise<Set<string> | undefined> {
  const extras = new Map<string, ResourcePickerExtra>();
  for (const resource of extraResources) extras.set(resource.path, resource);
  for (const path of current) {
    if (!extras.has(path)) {
      extras.set(path, { path, label: basename(path), type: resourceType });
    }
  }
  const payload = JSON.stringify({
    title: `${title} for this subagent`,
    resourceTypes: [resourceType],
    initialPaths: [...current],
    extraResources: [...extras.values()],
  });
  const raw = await ctx.ui.select(`${RESOURCE_PICKER_TITLE_PREFIX}${payload}`, [
    "Open Pi resource manager",
    "Cancel",
  ]);
  if (raw === undefined) return undefined;
  try {
    const paths: unknown = JSON.parse(raw);
    if (!Array.isArray(paths) || !paths.every((path) => typeof path === "string")) return current;
    return new Set(paths.map((path) => path.trim()).filter(Boolean));
  } catch {
    return current;
  }
}

function modelCatalog(ctx: ExtensionCommandContext): CatalogEntry[] {
  const models = ctx.modelRegistry.getAvailable?.() ?? ctx.modelRegistry.getAll();
  return models
    .map((model) => ({
      id: `${model.provider}/${model.id}`,
      label: `${model.provider}/${model.id}`,
      description: model.name,
    }))
    .sort((left, right) => left.label.localeCompare(right.label));
}

function updateSelection(
  definition: AgentDefinition,
  field: "tools" | "models" | "extensions" | "skills",
  values: Set<string>,
): void {
  const next = [...values].sort();
  if (field === "tools") {
    definition.tools = next;
  } else if (next.length === 0) {
    delete definition[field];
  } else {
    definition[field] = next;
  }
}

async function editDefinition(pi: ExtensionAPI, ctx: ExtensionCommandContext, definition: AgentDefinition): Promise<void> {
  const isBuiltinOverride = definition.builtin === true && definition.scope !== "builtin";
  const restoreChoice = definition.scope === "global" ? "Restore built-in defaults" : "Remove project override";
  while (true) {
    const choice = await ctx.ui.select(`Subagent ${definition.name} (${definition.scope})`, [
      definition.enabled ? "Disable" : "Enable",
      "Description",
      "Instructions",
      "Built-in tools",
      "Plugin tools",
      `Models (max ${MAX_AGENT_MODELS})`,
      "Extensions",
      "Skills",
      "Max turns",
      ...(isBuiltinOverride ? [restoreChoice] : ["Delete definition"]),
      "Save and close",
    ]);
    if (!choice || choice === "Save and close") {
      saveDefinition(ctx.cwd, definition);
      ctx.ui.notify(`Saved ${definition.scope} subagent ${definition.name}.`, "info");
      return;
    }
    if (choice === "Enable" || choice === "Disable") {
      definition.enabled = choice === "Enable";
      continue;
    }
    if (choice === restoreChoice || choice === "Delete definition") {
      const confirmed = await ctx.ui.confirm(
        choice === restoreChoice ? restoreChoice : "Delete subagent definition",
        choice === "Restore built-in defaults"
          ? `Remove the global override for ${definition.name} and restore the built-in profile?`
          : choice === "Remove project override"
            ? `Remove the project override for ${definition.name} and restore its inherited definition?`
          : `Delete the ${definition.scope} subagent definition ${definition.name}?`,
      );
      if (confirmed) {
        deleteDefinition(ctx.cwd, definition);
        ctx.ui.notify(
          choice === "Restore built-in defaults"
            ? `Restored built-in subagent ${definition.name}.`
            : choice === "Remove project override"
              ? `Removed project override for ${definition.name}.`
            : `Deleted ${definition.scope} subagent ${definition.name}.`,
          "info",
        );
        return;
      }
      continue;
    }
    if (choice === "Description") {
      const value = await ctx.ui.input("Subagent description", definition.description);
      if (value?.trim()) definition.description = value.trim();
      continue;
    }
    if (choice === "Instructions") {
      const value = await ctx.ui.editor("Subagent instructions", definition.systemPrompt);
      if (value !== undefined) definition.systemPrompt = value.trim();
      continue;
    }
    if (choice === "Built-in tools" || choice === "Plugin tools") {
      const catalog = toolCatalog(pi);
      const entries = choice === "Built-in tools" ? catalog.builtin : catalog.plugin;
      const allowed = new Set(entries.map((entry) => entry.id));
      const current = new Set((definition.tools ?? []).filter((tool) => allowed.has(tool)));
      const next = await selectToggles(ctx, choice, entries, current);
      if (next) {
        const preserved = (definition.tools ?? []).filter((tool) => !allowed.has(tool));
        updateSelection(definition, "tools", new Set([...preserved, ...next]));
      }
      continue;
    }
    if (choice.startsWith("Models")) {
      const entries = modelCatalog(ctx);
      const current = new Set((definition.models ?? []).filter((model) => entries.some((entry) => entry.id === model)));
      const next = await selectToggles(ctx, "Models", entries, current, MAX_AGENT_MODELS);
      if (next) updateSelection(definition, "models", next);
      continue;
    }
    if (choice === "Extensions") {
      const entries = extensionCatalog(pi);
      const current = new Set(definition.extensions ?? []);
      const next = await selectResources(
        ctx,
        "Extensions",
        "extensions",
        current,
        entries.map((entry) => ({ path: entry.id, label: entry.label, type: "extensions" })),
      );
      if (next) updateSelection(definition, "extensions", next);
      continue;
    }
    if (choice === "Skills") {
      const current = new Set(definition.skills ?? []);
      const next = await selectResources(ctx, "Skills", "skills", current);
      if (next) updateSelection(definition, "skills", next);
      continue;
    }
    const raw = await ctx.ui.input("Maximum turns (0 = unlimited)", String(definition.maxTurns ?? 0));
    if (raw === undefined) continue;
    const value = Number(raw.trim());
    if (!Number.isInteger(value) || value < 0) {
      ctx.ui.notify("Maximum turns must be a non-negative integer.", "warning");
    } else {
      definition.maxTurns = value;
    }
  }
}

async function configureSubagents(pi: ExtensionAPI, ctx: ExtensionCommandContext): Promise<void> {
  const definitions = [...loadAgentDefinitions(ctx.cwd).values()].sort((left, right) => left.name.localeCompare(right.name));
  const choices = [
    "New project subagent",
    "New global subagent",
    ...definitions.map((definition) => {
      const source = definition.scope === "builtin" ? "built-in" : definition.scope;
      const suffix = definition.scope === "builtin" ? " (default)" : definition.builtin ? " (built-in override)" : "";
      return `${source}: ${definition.name}${definition.enabled ? suffix : " (disabled)"}`;
    }),
  ];
  const choice = await ctx.ui.select("Subagent configuration", choices);
  if (!choice) return;
  if (choice.startsWith("New ")) {
    const scope: AgentDefinitionScope = choice.includes("project") ? "project" : "global";
    const rawName = await ctx.ui.input("Subagent name (letters, numbers, _ or -)");
    const name = rawName && definitionName(rawName);
    if (!name) {
      ctx.ui.notify("Subagent name is invalid or cancelled.", "warning");
      return;
    }
    await editDefinition(pi, ctx, {
      name,
      scope,
      enabled: true,
      description: name,
      systemPrompt: profileFor("general-purpose").systemPrompt,
    });
    return;
  }
  const target = definitions.find((definition) => {
    const source = definition.scope === "builtin" ? "built-in" : definition.scope;
    return choice.startsWith(`${source}: ${definition.name}`);
  });
  if (!target) return;
  if (target.scope === "builtin") {
    const scopeChoice = await ctx.ui.select(`Create override for built-in ${target.name}`, [
      "Project override",
      "Global override",
    ]);
    if (!scopeChoice) return;
    await editDefinition(pi, ctx, {
      ...cloneDefinition(target),
      scope: scopeChoice === "Project override" ? "project" : "global",
      builtin: true,
    });
    return;
  }
  if (target.scope === "global") {
    const action = await ctx.ui.select(`Global subagent ${target.name}`, [
      "Edit global definition",
      "Disable in this project",
    ]);
    if (action === "Disable in this project") {
      saveDefinition(ctx.cwd, {
        ...cloneDefinition(target),
        scope: "project",
        enabled: false,
      });
      ctx.ui.notify(`Disabled global subagent ${target.name} in this project.`, "info");
      return;
    }
    if (action !== "Edit global definition") return;
  }
  await editDefinition(pi, ctx, cloneDefinition(target));
}

type ChildUpdate =
  | { type: "assistant_delta"; text: string }
  | { type: "thinking_delta"; text: string }
  | { type: "user"; text: string }
  | { type: "tool_call"; toolCallId: string; toolName: string; args: unknown }
  | { type: "tool_update"; toolCallId: string; toolName: string; partialResult: unknown }
  | { type: "tool_result"; toolCallId: string; toolName: string; result: unknown; isError: boolean };

type BridgeKind = "spawned" | "progress" | "child_update" | "finished";

type BridgeEnvelope = {
  version: 1;
  sequence: number;
  replay: boolean;
  kind: BridgeKind;
  parentSessionId: string;
  subagentId: string;
  childSessionId: string;
  payload: Record<string, unknown>;
};

type PersistedRecord = {
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
};

type SubagentRecord = {
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
  cancelRequested: boolean;
  /** Max turns before injecting a summary prompt. 0 = unlimited. */
  maxTurns: number;
  /** Set when turn limit triggers abort-then-summarize. */
  turnLimitReached: boolean;
  /** Resolved when finish() is called — enables true blocking wait. */
  donePromise: Promise<void>;
  doneResolve: () => void;
  progressTimer: ReturnType<typeof setInterval>;
  removeAbortListener: () => void;
  unsubscribe: () => void;
};

function requireText(value: unknown, name: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${name} is required`);
  }
  return value.trim();
}

function requireCapability(value: string | undefined): CapabilityMode {
  const capability = value ?? "all";
  if (!(capability in CAPABILITY_TOOLS)) {
    throw new Error(`unsupported capability_mode: ${capability}`);
  }
  return capability as CapabilityMode;
}

function resolveProfile(type: string, capabilityMode: string | undefined): {
  type: string;
  capabilityMode: CapabilityMode;
  systemPrompt: string;
} {
  const normalizedType = type.trim() || "general-purpose";
  const profile = AGENT_PROFILES[normalizedType] ?? AGENT_PROFILES["general-purpose"];
  return {
    type: normalizedType,
    capabilityMode: requireCapability(capabilityMode ?? profile.capabilityMode),
    systemPrompt: profile.systemPrompt,
  };
}

function textFromContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((block): block is { type: string; text?: unknown } => typeof block === "object" && block !== null)
    .filter((block) => block.type === "text" && typeof block.text === "string")
    .map((block) => block.text as string)
    .join("");
}

function lastAssistantText(session: AgentSession): string {
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

function extractUsage(message: unknown): number {
  if (typeof message !== "object" || message === null) return 0;
  const usage = (message as { usage?: unknown }).usage;
  if (typeof usage !== "object" || usage === null) return 0;
  const input = (usage as { input?: unknown }).input;
  const output = (usage as { output?: unknown }).output;
  return (typeof input === "number" ? input : 0) + (typeof output === "number" ? output : 0);
}

function childUpdate(event: AgentSessionEvent): ChildUpdate | undefined {
  if (event.type === "message_update") {
    if (event.assistantMessageEvent.type === "text_delta") {
      return { type: "assistant_delta", text: event.assistantMessageEvent.delta };
    }
    if (event.assistantMessageEvent.type === "thinking_delta") {
      return { type: "thinking_delta", text: event.assistantMessageEvent.delta };
    }
  }
  if (event.type === "tool_execution_start") {
    return {
      type: "tool_call",
      toolCallId: event.toolCallId,
      toolName: event.toolName,
      args: event.args,
    };
  }
  if (event.type === "tool_execution_update") {
    return {
      type: "tool_update",
      toolCallId: event.toolCallId,
      toolName: event.toolName,
      partialResult: event.partialResult,
    };
  }
  if (event.type === "tool_execution_end") {
    return {
      type: "tool_result",
      toolCallId: event.toolCallId,
      toolName: event.toolName,
      result: event.result,
      isError: event.isError,
    };
  }
  return undefined;
}

export default function piGrokSubagents(pi: ExtensionAPI): void {
  if (process.env.PI_GROK_SUBAGENTS !== "1") return;

  const records = new Map<string, SubagentRecord>();
  const queuedBackground: Array<{ record: SubagentRecord; prompt: string }> = [];
  let runningBackground = 0;
  let nextSequence = 1;

  const publishTodoBacking = () => {
    const count = [...records.values()].filter((record) => record.background && !record.finished).length;
    pi.events.emit("pi-grok:todo-backing", { source: "subagent", count });
  };

  function emit(
    record: Pick<SubagentRecord, "id" | "childSessionId" | "parentSessionId">,
    kind: BridgeKind,
    payload: Record<string, unknown>,
    replay = false,
  ): void {
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
    if (replay) {
      // Replay runs during session_start. Keep the existing message shape for
      // the adapter, but do not append replay records to the active session.
      pi.sendMessage(
        {
          customType: BRIDGE_TYPE,
          content: "",
          display: false,
          details: envelope,
        },
        { triggerTurn: false },
      );
      return;
    }
    // Live bridge traffic is session/TUI state, not an LLM message. Using
    // sendMessage() here would steer the parent while it is streaming, even
    // with triggerTurn:false, causing child deltas to create phantom turns.
    pi.appendEntry(BRIDGE_TYPE, envelope);
  }

  function persistedRecord(entry: unknown): PersistedRecord | undefined {
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
      typeof value.tokensUsed !== "number"
    ) {
      return undefined;
    }
    return value as PersistedRecord;
  }

  function replayChildTranscript(snapshot: PersistedRecord): void {
    let entries: readonly unknown[];
    try {
      entries = SessionManager.open(snapshot.childSessionFile).getBranch();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      emit(snapshot, "finished", {
        status: "failed",
        durationMs: 0,
        turns: snapshot.turnCount,
        toolCalls: snapshot.toolCallCount,
        tokensUsed: snapshot.tokensUsed,
        error: `child transcript is unavailable: ${message}`,
      }, true);
      return;
    }

    for (const entry of entries) {
      const message = (entry as { type?: unknown; message?: unknown }).message;
      if (typeof message !== "object" || message === null) continue;
      const childMessage = message as {
        role?: unknown;
        content?: unknown;
        toolCallId?: unknown;
        toolName?: unknown;
        isError?: unknown;
      };
      if (childMessage.role === "user") {
        emit(snapshot, "child_update", {
          update: { type: "user", text: textFromContent(childMessage.content) },
        }, true);
        continue;
      }
      if (childMessage.role === "assistant" && Array.isArray(childMessage.content)) {
        for (const block of childMessage.content) {
          if (typeof block !== "object" || block === null) continue;
          const value = block as { type?: unknown; text?: unknown; thinking?: unknown; id?: unknown; name?: unknown; arguments?: unknown };
          if (value.type === "text" && typeof value.text === "string") {
            emit(snapshot, "child_update", { update: { type: "assistant_delta", text: value.text } }, true);
          } else if (value.type === "thinking" && typeof value.thinking === "string") {
            emit(snapshot, "child_update", { update: { type: "thinking_delta", text: value.thinking } }, true);
          } else if (value.type === "toolCall" && typeof value.id === "string" && typeof value.name === "string") {
            emit(snapshot, "child_update", {
              update: { type: "tool_call", toolCallId: value.id, toolName: value.name, args: value.arguments ?? {} },
            }, true);
          }
        }
        continue;
      }
      if (
        childMessage.role === "toolResult" &&
        typeof childMessage.toolCallId === "string" &&
        typeof childMessage.toolName === "string"
      ) {
        emit(snapshot, "child_update", {
          update: {
            type: "tool_result",
            toolCallId: childMessage.toolCallId,
            toolName: childMessage.toolName,
            result: { content: childMessage.content },
            isError: childMessage.isError === true,
          },
        }, true);
      }
    }
  }

  function latestPersistedRecords(ctx: ExtensionContext, allBranches = false): PersistedRecord[] {
    const latest = new Map<string, PersistedRecord>();
    const parentSessionId = ctx.sessionManager.getSessionId();
    const entries = allBranches ? ctx.sessionManager.getEntries() : ctx.sessionManager.getBranch();
    for (const entry of entries) {
      const snapshot = persistedRecord(entry);
      if (snapshot?.parentSessionId === parentSessionId) latest.set(snapshot.id, snapshot);
    }
    return [...latest.values()].sort((left, right) => left.startedAt - right.startedAt);
  }

  function persistedStatusLabel(snapshot: PersistedRecord): string {
    if (snapshot.status === "running" && !records.has(snapshot.id)) return "INTERRUPTED";
    return snapshot.status.toUpperCase();
  }

  function resolvePersistedSubagent(snapshots: PersistedRecord[], id: string): PersistedRecord {
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

  function formatPersistedSubagentHistory(snapshot: PersistedRecord, snapshots: PersistedRecord[]): string {
    const branch = SessionManager.open(snapshot.childSessionFile).getBranch();
    const shortId = shortSubagentIdFor(snapshot.id, snapshots.map((candidate) => candidate.id));
    const lines = [
      `# Subagent ${shortId}: ${snapshot.description}`,
      `Status: ${persistedStatusLabel(snapshot)} · ${snapshot.type} · ${snapshot.turnCount} turns · ${snapshot.toolCallCount} tools`,
      `Child session: ${snapshot.childSessionId}`,
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

  async function showSubagentHistory(args: string, ctx: ExtensionCommandContext): Promise<void> {
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
        return `${shortId} · [${persistedStatusLabel(candidate)}] ${candidate.description}`;
      });
      const selected = await ctx.ui.select("Subagent history", choices);
      if (!selected) return;
      const selectedId = selected.split(" · ", 1)[0];
      snapshot = resolvePersistedSubagent(snapshots, selectedId);
    }

    try {
      const ids = snapshots.map((candidate) => candidate.id);
      const shortId = shortSubagentIdFor(snapshot.id, ids);
      const history = formatPersistedSubagentHistory(snapshot, snapshots);
      await ctx.ui.editor(`Subagent ${shortId} history (changes are ignored)`, history);
    } catch (error) {
      ctx.ui.notify(`Unable to open subagent history: ${error instanceof Error ? error.message : String(error)}`, "error");
    }
  }

  function replayPersistedRecords(ctx: ExtensionContext): void {
    for (const snapshot of latestPersistedRecords(ctx, true)) {
      emit(snapshot, "spawned", {
        parentToolCallId: snapshot.parentToolCallId,
        description: snapshot.description,
        subagentType: snapshot.type,
        background: snapshot.background,
        capabilityMode: snapshot.capabilityMode,
        model: snapshot.modelId,
        prompt: snapshot.prompt,
      }, true);
      replayChildTranscript(snapshot);
      const status = snapshot.status === "running" ? "cancelled" : snapshot.status;
      emit(snapshot, "finished", {
        status,
        durationMs: Math.max(0, Date.now() - snapshot.startedAt),
        turns: snapshot.turnCount,
        toolCalls: snapshot.toolCallCount,
        tokensUsed: snapshot.tokensUsed,
        error: snapshot.status === "running" ? "Pi host restarted before child completion" : undefined,
      }, true);
    }
  }

  function persist(record: SubagentRecord, status: PersistedRecord["status"]): void {
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
    };
    pi.appendEntry(STATE_ENTRY_TYPE, snapshot);
  }

  function emitProgress(record: SubagentRecord): void {
    if (record.finished) return;
    emit(record, "progress", {
      durationMs: Date.now() - record.startedAt,
      turnCount: record.turnCount,
      toolCallCount: record.toolCallCount,
      toolsUsed: [...record.toolsUsed],
      errorCount: record.errorCount,
      tokensUsed: record.tokensUsed,
    });
  }

  function finish(record: SubagentRecord, status: "completed" | "failed" | "cancelled", error?: string): void {
    if (record.finished) return;
    record.finished = true;
    publishTodoBacking();
    record.terminalStatus = status;
    if (error) record.lastError = error;
    clearInterval(record.progressTimer);
    record.removeAbortListener();
    record.unsubscribe();
    record.doneResolve();
    persist(record, status);
    emit(record, "finished", {
      status,
      durationMs: Date.now() - record.startedAt,
      turns: record.turnCount,
      toolCalls: record.toolCallCount,
      tokensUsed: record.tokensUsed,
      error,
      output: lastAssistantText(record.session),
    });
  }

  async function createRecord(
    toolCallId: string,
    params: {
      prompt: string;
      description: string;
      subagent_type?: string;
      background?: boolean;
      capability_mode?: string;
      model?: string;
      max_turns?: number;
    },
    signal: AbortSignal | undefined,
    ctx: ExtensionContext,
  ): Promise<SubagentRecord> {
    const prompt = requireText(params.prompt, "prompt");
    const description = requireText(params.description, "description");
    const requestedType = params.subagent_type || "general-purpose";
    const definition = selectedDefinition(ctx.cwd, requestedType);
    if (definition && !definition.enabled) {
      throw new Error(`Subagent type \"${definition.name}\" is disabled by its ${definition.scope} Markdown definition.`);
    }
    const profile = profileFor(requestedType, params.capability_mode);
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
    const resourceLoader = new DefaultResourceLoader({
      cwd: ctx.cwd,
      agentDir,
      // Only resources explicitly selected in the product-isolated agent
      // definition are loaded. This includes grok-pi's own injected extension
      // temp files when the user selects them from the Pi-provided catalog.
      noExtensions: true,
      noSkills: true,
      additionalExtensionPaths: definition?.extensions ?? [],
      additionalSkillPaths: definition?.skills ?? [],
      noPromptTemplates: true,
      noThemes: true,
      noContextFiles: true,
      systemPromptOverride: () => definition?.systemPrompt || profile.systemPrompt,
      appendSystemPromptOverride: () => [],
    });
    await resourceLoader.reload();

    const { session } = await createAgentSession({
      cwd: ctx.cwd,
      agentDir,
      sessionManager: SessionManager.create(ctx.cwd, join(dirname(parentSessionFile), "subagent")),
      settingsManager,
      model,
      tools: definition?.tools ?? [...CAPABILITY_TOOLS[profile.capabilityMode]],
      resourceLoader,
    });
    await session.bindExtensions({});
    // `createAgentSession` retains this allowlist through extension binding;
    // set it once more after dynamic extension tools are registered so an
    // absent/removed configured plugin tool cannot leak into the child.
    session.setActiveToolsByName(definition?.tools ?? [...CAPABILITY_TOOLS[profile.capabilityMode]]);

    const childSessionFile = session.sessionFile;
    if (!childSessionFile) throw new Error("child session persistence is unavailable");
    const parentSessionId = ctx.sessionManager.getSessionId();
    const id = randomUUID();
    let doneResolve!: () => void;
    const donePromise = new Promise<void>((resolve) => { doneResolve = resolve; });
    const record = {
      id,
      childSessionId: session.sessionId,
      childSessionFile,
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
      cancelRequested: false,
      maxTurns: definition?.maxTurns ?? params.max_turns ?? 0,
      turnLimitReached: false,
      donePromise,
      doneResolve,
    } as Omit<SubagentRecord, "progressTimer" | "removeAbortListener" | "unsubscribe">;

    const unsubscribe = session.subscribe((event) => {
      if (event.type === "turn_end") {
        record.turnCount += 1;
        // Turn limit reached: steer the agent with a summary prompt.
        // "steer" interrupts the current turn and injects the message,
        // giving the agent a chance to produce final output naturally.
        if (record.maxTurns > 0 && record.turnCount >= record.maxTurns && !record.turnLimitReached) {
          record.turnLimitReached = true;
          const summaryPrompt =
            "[SYSTEM] You have reached the maximum number of turns allowed (" +
            String(record.maxTurns) +
            "). Stop all further tool calls immediately. " +
            "Produce your final summary now — return a concise, evidence-based result " +
            "of everything you have gathered so far. Do not make any more tool calls.";
          void session.prompt(summaryPrompt, { streamingBehavior: "steer" }).catch(() => undefined);
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
      const update = childUpdate(event);
      if (update) emit(record, "child_update", { update });
    });

    const onAbort = () => {
      record.cancelRequested = true;
      session.abort();
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    const removeAbortListener = () => signal?.removeEventListener("abort", onAbort);
    const progressTimer = setInterval(() => emitProgress(record as SubagentRecord), PROGRESS_INTERVAL_MS);
    const completeRecord: SubagentRecord = { ...record, progressTimer, removeAbortListener, unsubscribe };

    records.set(id, completeRecord);
    if (completeRecord.background) publishTodoBacking();
    persist(completeRecord, "running");
    emit(completeRecord, "spawned", {
      parentToolCallId: toolCallId,
      description,
      subagentType: completeRecord.type,
      background: completeRecord.background,
      capabilityMode: profile.capabilityMode,
      model: model.id,
      prompt,
    });
    emit(completeRecord, "child_update", { update: { type: "user", text: prompt } });
    return completeRecord;
  }

  async function run(record: SubagentRecord, prompt: string): Promise<string> {
    try {
      await record.session.prompt(prompt);
      const output = lastAssistantText(record.session);
      finish(record, "completed");
      return output;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      finish(record, record.cancelRequested ? "cancelled" : "failed", message);
      throw error;
    }
  }

  pi.on("session_start", (_event, ctx) => {
    replayPersistedRecords(ctx);
    publishTodoBacking();
  });

  function scheduleBackground(record: SubagentRecord, prompt: string): void {
    if (runningBackground >= MAX_BACKGROUND_CONCURRENCY) {
      queuedBackground.push({ record, prompt });
      return;
    }
    runningBackground += 1;
    void run(record, prompt)
      .catch(() => undefined)
      .finally(() => {
        runningBackground -= 1;
        const next = queuedBackground.shift();
        if (next) scheduleBackground(next.record, next.prompt);
      });
  }

  type MessageDelivery = "steer" | "follow_up";

  function shortSubagentIdFor(id: string, candidateIds: Iterable<string>): string {
    const candidates = [...candidateIds];
    for (let length = SHORT_SUBAGENT_ID_LENGTH; length < id.length; length += 1) {
      const prefix = id.slice(0, length);
      const matches = candidates.filter((candidate) => candidate.startsWith(prefix));
      if (matches.length <= 1) return prefix;
    }
    return id;
  }

  function shortSubagentId(id: string): string {
    return shortSubagentIdFor(id, records.keys());
  }

  function matchingSubagents(id: string): SubagentRecord[] {
    const exact = records.get(id);
    if (exact) return [exact];
    if (id.length < SHORT_SUBAGENT_ID_LENGTH) return [];
    return [...records.values()].filter((record) => record.id.startsWith(id));
  }

  function tryResolveSubagent(id: string): SubagentRecord | undefined {
    const matches = matchingSubagents(id);
    return matches.length === 1 ? matches[0] : undefined;
  }

  function resolveSubagent(id: string): SubagentRecord {
    const matches = matchingSubagents(id);
    if (matches.length === 1) return matches[0];
    if (matches.length > 1) {
      throw new Error(`ambiguous subagent ID prefix: ${id}; use more characters`);
    }
    if (id.length < SHORT_SUBAGENT_ID_LENGTH) {
      throw new Error(`subagent ID prefix is too short: ${id}; use at least ${SHORT_SUBAGENT_ID_LENGTH} characters`);
    }
    throw new Error(`unknown subagent: ${id}. Use list_subagents to see available subagents.`);
  }

  function runningSubagent(id: string): SubagentRecord {
    const record = resolveSubagent(id);
    if (record.finished) throw new Error(`subagent ${shortSubagentId(record.id)} has already finished (${statusLabel(record)})`);
    return record;
  }

  function sendMessage(record: SubagentRecord, message: string, delivery: MessageDelivery): void {
    const streamingBehavior = delivery === "steer" ? "steer" : "followUp";
    emit(record, "child_update", { update: { type: "user", text: message } });
    void record.session.prompt(message, { streamingBehavior }).catch((error: unknown) => {
      const detail = error instanceof Error ? error.message : String(error);
      record.lastError = `Message delivery failed: ${detail}`;
      emitProgress(record);
    });
  }

  async function sendMessageFromCommand(args: string, ctx: ExtensionCommandContext): Promise<void> {
    const running = [...records.values()].filter((record) => !record.finished);
    if (running.length === 0) {
      ctx.ui.notify("No running subagents can receive a message.", "warning");
      return;
    }
    const supplied = args.trim();
    const [candidateId, ...candidateMessage] = supplied.split(/\s+/).filter(Boolean);
    let record: SubagentRecord | undefined;
    let message = supplied;
    if (candidateId) {
      record = tryResolveSubagent(candidateId);
      if (record) {
        message = candidateMessage.join(" ");
      } else if (/^[0-9a-f-]{8,}$/i.test(candidateId)) {
        try {
          resolveSubagent(candidateId);
        } catch (error) {
          ctx.ui.notify(error instanceof Error ? error.message : String(error), "error");
          return;
        }
      }
    }
    if (!record) {
      const choices = running.map((item) => {
        const shortId = shortSubagentId(item.id);
        return { id: item.id, label: `${shortId} · ${item.description}` };
      });
      const selected = await ctx.ui.select("Send message to subagent", choices.map((choice) => choice.label));
      if (!selected) return;
      const choice = choices.find((candidate) => candidate.label === selected);
      record = choice ? records.get(choice.id) : undefined;
      if (!record) return;
    }
    if (record.finished) {
      ctx.ui.notify(`Subagent ${shortSubagentId(record.id)} has already finished.`, "warning");
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
    sendMessage(record, message, delivery);
    ctx.ui.notify(`Sent ${delivery === "steer" ? "steer" : "follow-up"} message to ${record.description}.`, "info");
  }

  // ---------------------------------------------------------------------------
  // Helpers for output formatting
  // ---------------------------------------------------------------------------

  function formatDuration(ms: number): string {
    if (ms < 1000) return `${ms}ms`;
    const seconds = Math.floor(ms / 1000);
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    return `${minutes}m${seconds % 60}s`;
  }

  function statusLabel(record: SubagentRecord): string {
    if (!record.finished) return "RUNNING";
    return (record.terminalStatus ?? "completed").toUpperCase();
  }

  function formatSubagentResult(record: SubagentRecord): string {
    const elapsed = formatDuration(Date.now() - record.startedAt);
    const status = statusLabel(record);
    const shortId = shortSubagentId(record.id);
    const header = `[${status}] ${record.description} (${shortId}) — ${elapsed}, ${record.turnCount} turns, ${record.toolCallCount} tool calls`;
    const historyHint = `History: /subagent-history ${shortId}`;
    if (!record.finished) {
      const tools = [...record.toolsUsed].join(", ") || "none yet";
      return `${header}\n${historyHint}\nStatus: still running. Tools used: ${tools}. Tokens: ${record.tokensUsed}.\nUse get_command_or_subagent_output with timeout_ms to wait for completion.`;
    }
    const output = lastAssistantText(record.session);
    const errorLine = record.lastError ? `\nError: ${record.lastError}` : "";
    if (!output) return `${header}${errorLine}\n${historyHint}\n(Subagent completed without text output.)`;
    // Truncate very long outputs to avoid flooding parent context
    const MAX_OUTPUT_CHARS = 12_000;
    const truncated = output.length > MAX_OUTPUT_CHARS
      ? `${output.slice(0, MAX_OUTPUT_CHARS)}\n\n… [truncated ${output.length - MAX_OUTPUT_CHARS} chars]`
      : output;
    return `${header}${errorLine}\n${historyHint}\n\n${truncated}`;
  }

  function waitForRecords(ids: string[], timeoutMs: number, signal?: AbortSignal): Promise<void> {
    const promises = ids.map((id) => {
      const r = records.get(id);
      return r ? r.donePromise : Promise.resolve();
    });
    const all = Promise.all(promises).then(() => undefined);
    if (timeoutMs <= 0) return all;
    const timeout = new Promise<void>((resolve) => {
      const timer = setTimeout(resolve, timeoutMs);
      signal?.addEventListener("abort", () => { clearTimeout(timer); resolve(); }, { once: true });
    });
    return Promise.race([all, timeout]);
  }

  // ---------------------------------------------------------------------------
  // Native Pager configuration entry point
  // ---------------------------------------------------------------------------

  pi.registerCommand("subagents", {
    description: "Configure project/global Pi subagents",
    handler: async (_args, ctx) => {
      await configureSubagents(pi, ctx);
    },
  });

  pi.registerCommand("subagent-message", {
    description: "Send a steer or follow-up message to a running Pi subagent",
    handler: async (args, ctx) => {
      await sendMessageFromCommand(args, ctx);
    },
  });

  pi.registerCommand("subagent-history", {
    description: "View the persisted transcript for a current or finished Pi subagent",
    handler: async (args, ctx) => {
      await showSubagentHistory(args, ctx);
    },
  });

  // ---------------------------------------------------------------------------
  // Tool: spawn_subagent
  // ---------------------------------------------------------------------------

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
      capability_mode: Type.Optional(
        Type.String({ description: "Tool access: read-only, read-write, execute, or all. Defaults to profile capability." }),
      ),
    }),
    async execute(toolCallId, params, signal, _onUpdate, ctx) {
      const record = await createRecord(toolCallId, params, signal, ctx);
      if (record.background) {
        scheduleBackground(record, params.prompt);
        return {
          content: [{ type: "text", text: `Started background subagent ${shortSubagentId(record.id)}.\nUse get_command_or_subagent_output with task_ids=["${shortSubagentId(record.id)}"] and timeout_ms to wait for its result.\nHistory: /subagent-history ${shortSubagentId(record.id)}` }],
          details: { subagentId: record.id, childSessionId: record.childSessionId, background: true },
        };
      }
      const output = await run(record, params.prompt);
      return {
        content: [{ type: "text", text: `${output || "Subagent completed without text output."}\n\nHistory: /subagent-history ${shortSubagentId(record.id)}` }],
        details: { subagentId: record.id, childSessionId: record.childSessionId, background: false },
      };
    },
  });

  // ---------------------------------------------------------------------------
  // Tool: send_message_to_subagent
  // ---------------------------------------------------------------------------

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
      delivery: Type.Optional(
        Type.Union([
          Type.Literal("follow_up"),
          Type.Literal("steer"),
        ], { description: "follow_up queues after the current turn; steer interrupts it. Defaults to follow_up." }),
      ),
    }),
    async execute(_toolCallId, params) {
      const id = requireText(params.task_id ?? params.subagent_id, "task_id or subagent_id");
      const message = requireText(params.message, "message");
      const delivery: MessageDelivery = params.delivery === "steer" ? "steer" : "follow_up";
      const record = runningSubagent(id);
      sendMessage(record, message, delivery);
      return {
        content: [{
          type: "text",
          text: `Queued ${delivery === "steer" ? "steer" : "follow-up"} message for subagent ${shortSubagentId(record.id)}.`,
        }],
        details: { subagentId: record.id, delivery, accepted: true },
      };
    },
  });

  // ---------------------------------------------------------------------------
  // Tool: get_command_or_subagent_output
  // ---------------------------------------------------------------------------

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
      // Accept both task_ids array and legacy subagent_id single string.
      const requestedIds: string[] = params.task_ids?.length
        ? params.task_ids
        : params.subagent_id
          ? [params.subagent_id]
          : [];
      if (requestedIds.length === 0) throw new Error("Provide task_ids (array) or subagent_id (string) with at least one subagent ID");

      const resolvedRecords = requestedIds.map((id) => resolveSubagent(id));
      const ids = resolvedRecords.map((record) => record.id);

      // Blocking wait if timeout_ms > 0. The waiter receives canonical full IDs.
      const timeoutMs = Math.min(Math.max(params.timeout_ms ?? 0, 0), MAX_WAIT_MS);
      if (timeoutMs > 0) {
        await waitForRecords(ids, timeoutMs, signal);
      }

      const results = resolvedRecords.map((record) => formatSubagentResult(record));
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
            status: statusLabel(record),
            turns: record.turnCount,
            toolCalls: record.toolCallCount,
          })),
        },
      };
    },
  });

  // ---------------------------------------------------------------------------
  // Tool: kill_command_or_subagent
  // ---------------------------------------------------------------------------

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
      const record = resolveSubagent(id);
      if (record.finished) {
        return {
          content: [{ type: "text", text: `Subagent ${shortSubagentId(record.id)} already finished (${statusLabel(record)}).` }],
          details: { subagentId: record.id, finished: true },
        };
      }
      record.cancelRequested = true;
      record.session.abort();
      return {
        content: [{ type: "text", text: `Cancelled subagent ${shortSubagentId(record.id)} (${record.description}).` }],
        details: { subagentId: record.id, finished: false },
      };
    },
  });

  // ---------------------------------------------------------------------------
  // Tool: list_subagents
  // ---------------------------------------------------------------------------

  pi.registerTool({
    name: "list_subagents",
    label: "List Subagents",
    description: "List all subagents in this session with their current status, progress, and IDs.",
    parameters: Type.Object({}),
    async execute() {
      if (records.size === 0) {
        return {
          content: [{ type: "text", text: "No subagents have been spawned in this session." }],
          details: { subagents: [] },
        };
      }
      const lines = [...records.values()]
        .sort((a, b) => a.startedAt - b.startedAt)
        .map((r) => {
          const elapsed = formatDuration(Date.now() - r.startedAt);
          const status = statusLabel(r);
          const bg = r.background ? "bg" : "fg";
          const shortId = shortSubagentId(r.id);
          return `• [${status}] ${shortId} "${r.description}" (${bg}, ${r.type}) — ${elapsed}, ${r.turnCount} turns, ${r.toolCallCount} tools — /subagent-history ${shortId}`;
        });
      return {
        content: [{ type: "text", text: `Subagents (${records.size}):\n${lines.join("\n")}` }],
        details: {
          subagents: [...records.values()].map((record) => ({
            subagentId: record.id,
            finished: record.finished,
            status: statusLabel(record),
          })),
        },
      };
    },
  });

  // ---------------------------------------------------------------------------
  // Internal command (kept for backward compat with pager bridge)
  // ---------------------------------------------------------------------------

  pi.registerCommand("__pi_grok_subagent_cancel", {
    description: "Internal Pi-Grok bridge command: cancel a subagent",
    handler: async (args) => {
      const id = requireText(args, "subagent id");
      const record = resolveSubagent(id);
      if (!record.finished) {
        record.cancelRequested = true;
        record.session.abort();
      }
    },
  });

  pi.on("session_shutdown", () => {
    for (const record of records.values()) {
      if (!record.finished) {
        record.cancelRequested = true;
        record.session.abort();
      }
    }
  });
}
