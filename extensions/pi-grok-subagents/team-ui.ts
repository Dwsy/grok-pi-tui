/**
 * Interactive team-preset editor for `/subagent-teams`.
 *
 * Discovery lives entirely in the TUI: available models appear as select
 * options and supported thinking levels are derived per selected model. Nothing
 * here registers a model-facing tool, so the model never sees the catalog.
 */

import type { ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { definitionName } from "./definitions.ts";
import {
  cloneTeamDefinition,
  deleteTeamDefinition,
  loadTeamDefinitions,
  newTeamDefinition,
  saveTeamDefinition,
  type TeamDefinition,
  type TeamDefinitionScope,
  type TeamMemberDefinition,
} from "./teams.ts";
import {
  clampThinkingLevel,
  supportedThinkingLevels,
  THINKING_LEVELS,
  type ThinkingCapableModel,
  type ThinkingLevel,
} from "./thinking.ts";

const INHERIT_LABEL = "Inherit Pi settings (default)";
const ALL_MODELS_LABEL = "All available models (default)";
const MAX_TEAM_MEMBERS = 8;

type ExternalScope = Exclude<TeamDefinitionScope, "bundled">;

type ModelEntry = {
  key: string;
  label: string;
  model: ThinkingCapableModel;
};

function scopeLabel(scope: TeamDefinitionScope): string {
  return scope === "bundled" ? "built-in" : scope;
}

function scopeSuffix(team: TeamDefinition): string {
  if (team.scope === "bundled") return " (default)";
  return team.enabled ? "" : " (disabled)";
}

function teamChoiceLabel(team: TeamDefinition): string {
  return `${scopeLabel(team.scope)}: ${team.name}${scopeSuffix(team)}`;
}

function memberSummary(member: TeamMemberDefinition): string {
  const model = member.model ?? "default model";
  const thinking = member.thinkingLevel ?? "inherit";
  return `${member.name} — ${member.agent} · ${model} · ${thinking}`;
}

/** Available chat models with the fields the thinking-level rule needs. */
function modelEntries(ctx: ExtensionCommandContext): ModelEntry[] {
  const models = ctx.modelRegistry.getAvailable?.() ?? ctx.modelRegistry.getAll();
  return models
    .map((model) => ({
      key: `${model.provider}/${model.id}`,
      label: `${model.provider}/${model.id}${model.reasoning ? "" : " (no reasoning)"}`,
      model: model as ThinkingCapableModel,
    }))
    .sort((left, right) => left.key.localeCompare(right.key));
}

function modelForKey(entries: ModelEntry[], key: string | undefined): ThinkingCapableModel | undefined {
  if (!key) return undefined;
  return entries.find((entry) => entry.key === key)?.model;
}

/**
 * Thinking-level options for one member. A member without an explicit model may
 * be resolved to any model at spawn time, so offer the union of the catalog
 * instead of a set that would hide levels some model accepts.
 */
function thinkingOptions(
  entries: ModelEntry[],
  modelKey: string | undefined,
  current: ThinkingLevel | undefined,
): Array<{ label: string; level: ThinkingLevel | undefined }> {
  const selected = modelForKey(entries, modelKey);
  const supported = selected
    ? supportedThinkingLevels(selected)
    : [...new Set(entries.flatMap((entry) => supportedThinkingLevels(entry.model)))];
  const levels = THINKING_LEVELS.filter((level) => supported.includes(level));
  return [
    { label: INHERIT_LABEL, level: undefined },
    ...levels.map((level) => ({
      label: level === current ? `${level} (current)` : level,
      level,
    })),
  ];
}

/**
 * Returns the chosen `provider/model` key, `""` for the "all models" default,
 * or `undefined` when the dialog was cancelled.
 */
async function pickModel(
  ctx: ExtensionCommandContext,
  entries: ModelEntry[],
  current: ThinkingLevel | undefined,
): Promise<string | undefined> {
  if (entries.length === 0) {
    ctx.ui.notify("No models are currently available in Pi.", "warning");
    return undefined;
  }
  const options = [ALL_MODELS_LABEL, ...entries.map((entry) => entry.label)];
  const choice = await ctx.ui.select("Member model", options);
  if (choice === undefined) return undefined;
  if (choice === ALL_MODELS_LABEL) return "";
  const entry = entries.find((candidate) => choice === candidate.label);
  if (!entry) return undefined;
  if (current && !supportedThinkingLevels(entry.model).includes(current)) {
    const clamped = clampThinkingLevel(entry.model, current);
    ctx.ui.notify(
      `${entry.key} does not support thinking level "${current}"; it will run at "${clamped}".`,
      "warning",
    );
  }
  return entry.key;
}

/**
 * Returns the chosen level, `null` for "inherit Pi settings", or `undefined`
 * when the dialog was cancelled.
 */
async function pickThinkingLevel(
  ctx: ExtensionCommandContext,
  entries: ModelEntry[],
  modelKey: string | undefined,
  current: ThinkingLevel | undefined,
): Promise<ThinkingLevel | undefined | null> {
  const options = thinkingOptions(entries, modelKey, current);
  const choice = await ctx.ui.select("Member thinking level", options.map((option) => option.label));
  if (choice === undefined) return undefined;
  const match =
    options.find((option) => choice === option.label) ??
    options.find((option) => choice.replace(" (current)", "") === option.label);
  return match ? match.level ?? null : undefined;
}

async function editMember(
  ctx: ExtensionCommandContext,
  entries: ModelEntry[],
  member: TeamMemberDefinition,
): Promise<void> {
  while (true) {
    const choice = await ctx.ui.select(`Team member ${member.name}`, [
      `Agent (${member.agent})`,
      `Model (${member.model ?? "default"})`,
      `Thinking level (${member.thinkingLevel ?? "inherit"})`,
      `Max turns (${member.maxTurns ?? "definition default"})`,
      `Description (${member.description ?? "none"})`,
      "Task template",
      "Done",
    ]);
    if (!choice || choice === "Done") return;

    if (choice.startsWith("Agent")) {
      const raw = await ctx.ui.input("Agent definition name", member.agent);
      if (raw === undefined) continue;
      const name = definitionName(raw);
      if (!name) {
        ctx.ui.notify("Agent definition name is invalid.", "warning");
        continue;
      }
      member.agent = name;
      continue;
    }
    if (choice.startsWith("Model")) {
      const next = await pickModel(ctx, entries, member.thinkingLevel);
      if (next === undefined) continue;
      if (next === "") delete member.model;
      else member.model = next;
      continue;
    }
    if (choice.startsWith("Thinking level")) {
      const next = await pickThinkingLevel(ctx, entries, member.model, member.thinkingLevel);
      if (next === undefined) continue;
      if (next === null) delete member.thinkingLevel;
      else member.thinkingLevel = next;
      continue;
    }
    if (choice.startsWith("Max turns")) {
      const raw = await ctx.ui.input(
        "Max turns for this member (empty = agent definition, 0 = unlimited)",
        member.maxTurns === undefined ? "" : String(member.maxTurns),
      );
      if (raw === undefined) continue;
      if (!raw.trim()) {
        delete member.maxTurns;
        continue;
      }
      const value = Number(raw.trim());
      if (!Number.isInteger(value) || value < 0) {
        ctx.ui.notify("Max turns must be a non-negative integer.", "warning");
        continue;
      }
      member.maxTurns = value;
      continue;
    }
    if (choice.startsWith("Description")) {
      const raw = await ctx.ui.input("Member description (optional)", member.description ?? "");
      if (raw === undefined) continue;
      if (raw.trim()) member.description = raw.trim();
      else delete member.description;
      continue;
    }
    const raw = await ctx.ui.editor("Task template", member.task ?? "Work on the team objective: {{task}}");
    if (raw === undefined) continue;
    if (raw.trim()) member.task = raw.trim();
    else delete member.task;
  }
}

async function editMembers(
  ctx: ExtensionCommandContext,
  entries: ModelEntry[],
  team: TeamDefinition,
): Promise<void> {
  while (true) {
    const choices = [
      ...(team.members.length < MAX_TEAM_MEMBERS ? ["Add member"] : []),
      ...team.members.map((member) => memberSummary(member)),
      "Back",
    ];
    const choice = await ctx.ui.select(`Members of ${team.name} (${team.members.length}/${MAX_TEAM_MEMBERS})`, choices);
    if (!choice || choice === "Back") return;

    if (choice === "Add member") {
      const raw = await ctx.ui.input("Member name (lowercase letters, digits, underscores)");
      if (raw === undefined) continue;
      const name = raw.trim();
      if (!name || name === "root" || !/^[a-z0-9][a-z0-9_]{0,63}$/.test(name)) {
        ctx.ui.notify("Member name must use lowercase letters, digits, and underscores; `root` is reserved.", "warning");
        continue;
      }
      if (team.members.some((member) => member.name === name)) {
        ctx.ui.notify(`Member ${name} already exists.`, "warning");
        continue;
      }
      const member: TeamMemberDefinition = { name, agent: "general-purpose" };
      team.members.push(member);
      continue;
    }

    const index = team.members.findIndex((member) => memberSummary(member) === choice);
    if (index < 0) continue;
    const member = team.members[index];
    const action = await ctx.ui.select(`Member ${member.name}`, ["Edit settings", "Remove member"]);
    if (action === "Remove member") {
      const confirmed = await ctx.ui.confirm("Remove team member", `Remove ${member.name} from ${team.name}?`);
      if (confirmed) team.members.splice(index, 1);
      continue;
    }
    if (action === "Edit settings") await editMember(ctx, entries, member);
  }
}

async function chooseScope(
  ctx: ExtensionCommandContext,
  title: string,
  allowProject: boolean,
): Promise<ExternalScope | undefined> {
  const options = allowProject ? ["Project override", "Global override"] : ["Global"];
  const choice = await ctx.ui.select(title, options);
  if (!choice) return undefined;
  return choice.startsWith("Project") ? "project" : "global";
}

async function createTeam(ctx: ExtensionCommandContext, scope: ExternalScope): Promise<void> {
  const rawName = await ctx.ui.input("Team name (letters, numbers, _ or -)");
  if (rawName === undefined) return;
  const name = rawName.trim();
  if (!/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(name)) {
    ctx.ui.notify("Team name is invalid or cancelled.", "warning");
    return;
  }
  const existing = loadTeamDefinitions(ctx.cwd).get(name.toLowerCase());
  if (existing && existing.scope === scope) {
    ctx.ui.notify(`A ${scope} team preset named ${name} already exists.`, "warning");
    return;
  }
  await editTeam(ctx, newTeamDefinition(ctx.cwd, scope, name));
}

async function editTeam(ctx: ExtensionCommandContext, team: TeamDefinition): Promise<void> {
  const entries = modelEntries(ctx);
  const isOverride = team.scope !== "bundled";
  while (true) {
    const choice = await ctx.ui.select(`Team ${team.name} (${scopeLabel(team.scope)})`, [
      team.enabled ? "Disable" : "Enable",
      `Description (${team.description})`,
      "Instructions",
      `Members (${team.members.length})`,
      ...(isOverride ? ["Delete preset"] : []),
      "Save and close",
    ]);
    if (!choice) return;

    if (choice === "Save and close") {
      if (team.enabled && team.members.length === 0) {
        ctx.ui.notify("An enabled team must define at least one member.", "warning");
        continue;
      }
      try {
        const path = saveTeamDefinition(ctx.cwd, team);
        ctx.ui.notify(`Saved ${scopeLabel(team.scope)} team preset ${team.name}.\n${path}`, "info");
      } catch (error) {
        const detail = error instanceof Error ? error.message : String(error);
        ctx.ui.notify(`Could not save team preset: ${detail}`, "error");
      }
      return;
    }
    if (choice === "Enable" || choice === "Disable") {
      team.enabled = choice === "Enable";
      continue;
    }
    if (choice.startsWith("Description")) {
      const raw = await ctx.ui.input("Team description", team.description);
      if (raw !== undefined && raw.trim()) team.description = raw.trim();
      continue;
    }
    if (choice === "Instructions") {
      const raw = await ctx.ui.editor("Team instructions", team.instructions ?? "");
      if (raw === undefined) continue;
      if (raw.trim()) team.instructions = raw.trim();
      else delete team.instructions;
      continue;
    }
    if (choice.startsWith("Members")) {
      await editMembers(ctx, entries, team);
      continue;
    }
    if (choice === "Delete preset") {
      const confirmed = await ctx.ui.confirm("Delete team preset", `Delete ${team.scope} preset ${team.name}?`);
      if (!confirmed) continue;
      deleteTeamDefinition(ctx.cwd, team);
      ctx.ui.notify(`Deleted ${team.scope} team preset ${team.name}.`, "info");
      return;
    }
  }
}

/**
 * `/subagent-teams` entry point: pick a preset, then edit it. Bundled presets
 * require an explicit project/global override so the shipped bundle stays
 * read-only.
 */
export async function configureTeamPresets(ctx: ExtensionCommandContext): Promise<void> {
  const teams = [...loadTeamDefinitions(ctx.cwd).values()].sort((left, right) => left.name.localeCompare(right.name));
  const choices = ["New project team", "New global team", ...teams.map(teamChoiceLabel)];
  const choice = await ctx.ui.select("Subagents V2 team presets", choices);
  if (!choice) return;

  if (choice === "New project team" || choice === "New global team") {
    await createTeam(ctx, choice === "New project team" ? "project" : "global");
    return;
  }

  const target = teams.find((team) => teamChoiceLabel(team) === choice);
  if (!target) return;

  if (target.scope === "bundled") {
    const scope = await chooseScope(ctx, `Create override for built-in team ${target.name}`, true);
    if (!scope) return;
    await editTeam(ctx, cloneTeamDefinition(ctx.cwd, target, scope));
    return;
  }
  if (target.scope === "global") {
    const action = await ctx.ui.select(`Global team preset ${target.name}`, [
      "Edit global preset",
      "Override in this project",
    ]);
    if (!action) return;
    if (action === "Override in this project") {
      await editTeam(ctx, cloneTeamDefinition(ctx.cwd, target, "project"));
      return;
    }
  }
  await editTeam(ctx, { ...target, members: target.members.map((member) => ({ ...member })) });
}
