/** Native Pager configuration UI for subagent Markdown definitions. */

import { basename } from "node:path";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import {
  BUILTIN_TOOL_SET,
  MAX_AGENT_MODELS,
  MULTI_SELECT_TITLE_PREFIX,
  RESOURCE_PICKER_TITLE_PREFIX,
  type CatalogEntry,
  type InjectedExtensionCatalogEntry,
  type ResourcePickerExtra,
} from "./shared.ts";
import {
  cloneDefinition,
  definitionName,
  deleteDefinition,
  loadAgentDefinitions,
  profileFor,
  saveDefinition,
  selectedDefinition,
  type AgentDefinition,
  type AgentDefinitionScope,
} from "./definitions.ts";

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

export function toolCatalog(pi: ExtensionAPI): { builtin: CatalogEntry[]; plugin: CatalogEntry[] } {
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

export function extensionCatalog(pi: ExtensionAPI): CatalogEntry[] {
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
    if (!choice) return;
    if (choice === "Save and close") {
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

export async function configureSubagents(pi: ExtensionAPI, ctx: ExtensionCommandContext): Promise<void> {
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
    const suffix = definition.scope === "builtin" ? " (default)" : definition.builtin ? " (built-in override)" : "";
    const label = `${source}: ${definition.name}${definition.enabled ? suffix : " (disabled)"}`;
    return choice === label;
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
