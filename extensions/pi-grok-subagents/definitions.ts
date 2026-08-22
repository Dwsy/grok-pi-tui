/** Subagent Markdown definitions: built-in profiles, load/save, profile resolution. */

import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, isAbsolute, join } from "node:path";
import { randomUUID } from "node:crypto";
import { parseFrontmatter } from "@earendil-works/pi-coding-agent";
import { MAX_AGENT_MODELS, requireCapability, type CapabilityMode } from "./shared.ts";

export const AGENT_PROFILES: Record<string, { capabilityMode: CapabilityMode; systemPrompt: string }> = {
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

export type AgentDefinitionScope = "builtin" | "global" | "project";

export type AgentDefinition = {
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

export function productProjectDir(cwd: string): string {
  const configured = process.env.GROK_PROJECT_DIR?.trim() || ".grok-pi";
  return isAbsolute(configured) ? configured : join(cwd, configured);
}

export function productGlobalDir(): string {
  return process.env.GROK_HOME?.trim() || join(homedir(), ".grok-pi");
}

function definitionDir(cwd: string, scope: AgentDefinitionScope): string {
  if (scope === "builtin") throw new Error("Built-in subagents do not have a definition directory.");
  return scope === "project" ? join(productProjectDir(cwd), "agents") : join(productGlobalDir(), "agents");
}

function definitionPath(cwd: string, scope: AgentDefinitionScope, name: string): string {
  return join(definitionDir(cwd, scope), `${name}.md`);
}

export function definitionName(value: string): string | undefined {
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
      // A malformed higher-priority definition shadows an inherited definition
      // of the same file-stem name instead of failing open to broader defaults.
      definitions.delete(name.toLowerCase());
    }
  }
}

export function loadAgentDefinitions(cwd: string): Map<string, AgentDefinition> {
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

export function saveDefinition(cwd: string, definition: AgentDefinition): void {
  if (definition.scope === "builtin") throw new Error("Choose project or global scope before saving a built-in subagent.");
  const dir = definitionDir(cwd, definition.scope);
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const destination = definitionPath(cwd, definition.scope, definition.name);
  const temporary = `${destination}.${process.pid}.${randomUUID()}.tmp`;
  writeFileSync(temporary, serializeDefinition(definition), { encoding: "utf8", mode: 0o600 });
  renameSync(temporary, destination);
}

export function deleteDefinition(cwd: string, definition: AgentDefinition): void {
  if (definition.scope === "builtin") return;
  const path = definitionPath(cwd, definition.scope, definition.name);
  if (existsSync(path)) unlinkSync(path);
}

export function cloneDefinition(definition: AgentDefinition): AgentDefinition {
  return {
    ...definition,
    tools: definition.tools?.slice(),
    models: definition.models?.slice(),
    extensions: definition.extensions?.slice(),
    skills: definition.skills?.slice(),
  };
}

export function resolveProfile(type: string, capabilityMode: string | undefined): {
  type: string;
  capabilityMode: CapabilityMode;
  systemPrompt: string;
} {
  const normalizedType = (type.trim() || "general-purpose").toLowerCase();
  const profile = AGENT_PROFILES[normalizedType] ?? AGENT_PROFILES["general-purpose"];
  return {
    type: normalizedType,
    capabilityMode: requireCapability(capabilityMode ?? profile.capabilityMode),
    systemPrompt: profile.systemPrompt,
  };
}

export function profileFor(type: string, capabilityMode?: string): { type: string; capabilityMode: CapabilityMode; systemPrompt: string } {
  return resolveProfile(type, capabilityMode);
}

export function selectedDefinition(cwd: string, type: string): AgentDefinition | undefined {
  return loadAgentDefinitions(cwd).get(type.trim().toLowerCase());
}
