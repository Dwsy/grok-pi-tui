/** External team preset discovery and validation for Subagents V2. */

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { definitionName, productGlobalDir, productProjectDir } from "./definitions.ts";

export type TeamDefinitionScope = "bundled" | "global" | "project";

export type TeamMemberDefinition = {
  name: string;
  agent: string;
  description?: string;
  task?: string;
  model?: string;
  maxTurns?: number;
};

export type TeamDefinition = {
  name: string;
  scope: TeamDefinitionScope;
  enabled: boolean;
  description: string;
  instructions?: string;
  members: TeamMemberDefinition[];
  sourcePath: string;
};

const MAX_TEAM_MEMBERS = 8;

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function teamName(value: unknown, fallback: string): string {
  const name = optionalString(value) ?? fallback;
  if (!/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(name)) throw new Error(`invalid team name: ${name}`);
  return name;
}

function memberPathName(value: unknown): string {
  const name = optionalString(value);
  if (!name || name === "root" || !/^[a-z0-9][a-z0-9_]{0,63}$/.test(name)) {
    throw new Error("team member name must use lowercase letters, digits, and underscores");
  }
  return name;
}

function parseMember(value: unknown): TeamMemberDefinition {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("team member must be an object");
  }
  const raw = value as Record<string, unknown>;
  const name = memberPathName(raw.name);
  const agent = definitionName(optionalString(raw.agent) ?? "general-purpose");
  if (!agent) throw new Error(`invalid agent definition name for team member ${name}`);
  const maxTurns = raw.max_turns === undefined
    ? undefined
    : typeof raw.max_turns === "number" && Number.isInteger(raw.max_turns) && raw.max_turns >= 0
      ? raw.max_turns
      : (() => { throw new Error(`max_turns for team member ${name} must be a non-negative integer`); })();
  return {
    name,
    agent,
    description: optionalString(raw.description),
    task: optionalString(raw.task),
    model: optionalString(raw.model),
    maxTurns,
  };
}

function parseTeamFile(path: string, scope: TeamDefinitionScope): TeamDefinition {
  const parsed = JSON.parse(readFileSync(path, "utf8")) as unknown;
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error("team preset must be a JSON object");
  }
  const raw = parsed as Record<string, unknown>;
  const fallbackName = basename(path, ".json");
  const name = teamName(raw.name, fallbackName);
  const enabled = raw.enabled !== false;
  const rawMembers = Array.isArray(raw.members) ? raw.members : [];
  const members = rawMembers.map(parseMember);
  if (enabled && members.length === 0) throw new Error(`team ${name} must define at least one member`);
  if (members.length > MAX_TEAM_MEMBERS) throw new Error(`team ${name} exceeds ${MAX_TEAM_MEMBERS} members`);
  if (new Set(members.map((member) => member.name)).size !== members.length) {
    throw new Error(`team ${name} contains duplicate member names`);
  }
  return {
    name,
    scope,
    enabled,
    description: optionalString(raw.description) ?? name,
    instructions: optionalString(raw.instructions),
    members,
    sourcePath: path,
  };
}

function loadTeamDir(scope: TeamDefinitionScope, dir: string, teams: Map<string, TeamDefinition>): void {
  if (!existsSync(dir)) return;
  let files: string[];
  try {
    files = readdirSync(dir).filter((file) => file.endsWith(".json")).sort();
  } catch {
    return;
  }
  for (const file of files) {
    try {
      const team = parseTeamFile(join(dir, file), scope);
      teams.set(team.name.toLowerCase(), team);
    } catch {
      // A malformed higher-priority preset shadows an inherited preset with
      // the same file-stem name instead of silently falling back to it.
      const fallbackName = definitionName(basename(file, ".json"));
      if (fallbackName) teams.delete(fallbackName.toLowerCase());
    }
  }
}

export function bundledTeamDir(): string {
  return join(dirname(fileURLToPath(import.meta.url)), "teams");
}

export function loadTeamDefinitions(cwd: string): Map<string, TeamDefinition> {
  const teams = new Map<string, TeamDefinition>();
  loadTeamDir("bundled", bundledTeamDir(), teams);
  loadTeamDir("global", join(productGlobalDir(), "teams"), teams);
  loadTeamDir("project", join(productProjectDir(cwd), "teams"), teams);
  return teams;
}

export function selectedTeam(cwd: string, name: string): TeamDefinition {
  const normalized = name.trim().toLowerCase();
  const team = loadTeamDefinitions(cwd).get(normalized);
  if (!team) throw new Error(`unknown team preset: ${name}. Use /subagent-teams to list available teams.`);
  if (!team.enabled) throw new Error(`team preset ${team.name} is disabled by its ${team.scope} definition`);
  return team;
}

export function renderTeamTemplate(
  template: string | undefined,
  values: { task: string; team: string; agentPath: string; parentPath: string },
): string {
  const source = template?.trim() || "Work on the team objective: {{task}}";
  return source
    .replaceAll("{{task}}", values.task)
    .replaceAll("{{team}}", values.team)
    .replaceAll("{{agent_path}}", values.agentPath)
    .replaceAll("{{parent_path}}", values.parentPath);
}
