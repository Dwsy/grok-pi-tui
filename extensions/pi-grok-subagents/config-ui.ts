/**
 * Subagent definition configuration — one full-screen Pi TUI form.
 *
 * The definition list, the definition editor, and every field editor run as
 * `ctx.ui.custom` pi-tui components (projected into Grok Pager by the Remote
 * TUI bridge) with full mouse and keyboard support. Field editors are
 * `SettingsList` submenus built from the shared `ui-primitives` panels.
 * Persistence stays in `definitions.ts`; this module only collects input.
 */

import { homedir } from "node:os";
import { basename, join } from "node:path";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { loadSkills } from "@earendil-works/pi-coding-agent";
import type { Component, SettingItem, TUI } from "@earendil-works/pi-tui";
import {
  cloneDefinition,
  definitionName,
  deleteDefinition,
  loadAgentDefinitions,
  profileFor,
  saveDefinition,
  type AgentDefinition,
  type AgentDefinitionScope,
} from "./definitions.ts";
import { backgroundConcurrency, editBackgroundConcurrency } from "./runtime-settings.ts";
import { BUILTIN_TOOL_SET, MAX_AGENT_MODELS, type CatalogEntry, type InjectedExtensionCatalogEntry } from "./shared.ts";
import {
  makeConfirmPanel,
  makeEditorPanel,
  makeInputPanel,
  makeMultiSelectPanel,
  openCustomPanel,
  Panel,
  piInput,
  piSelect,
  settingsListTheme,
  uiStyles,
} from "./ui-primitives.ts";
import { SettingsList, Text } from "@earendil-works/pi-tui";

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

export function skillCatalog(cwd: string): CatalogEntry[] {
  const { skills } = loadSkills({
    cwd,
    agentDir: join(homedir(), ".pi", "agent"),
    skillPaths: [],
    includeDefaults: true,
  });
  const byPath = new Map<string, CatalogEntry>();
  for (const skill of skills) {
    if (byPath.has(skill.filePath)) continue;
    byPath.set(skill.filePath, { id: skill.filePath, label: skill.name, description: skill.description });
  }
  return [...byPath.values()].sort((left, right) => left.label.localeCompare(right.label));
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

type FormDone = (result: "saved" | "deleted" | undefined) => void;

/**
 * One definition editor form: every row opens a pi-tui submenu panel and writes
 * the draft in place; only the explicit Save row persists to disk. Escape
 * discards (nothing was written before Save).
 */
function buildDefinitionForm(
  pi: ExtensionAPI,
  ctx: ExtensionCommandContext,
  tui: TUI,
  theme: { fg: (color: string, text: string) => string; bold?: (text: string) => string },
  definition: AgentDefinition,
  done: FormDone,
): Component {
  const styles = uiStyles(theme);
  const isBuiltinOverride = definition.builtin === true && definition.scope !== "builtin";
  const restoreLabel = definition.scope === "global" ? "Restore built-in defaults" : "Remove project override";

  const catalog = toolCatalog(pi);
  const models = modelCatalog(ctx);

  const toolCount = (entries: CatalogEntry[]): string =>
    `${(definition.tools ?? []).filter((tool) => entries.some((entry) => entry.id === tool)).length} enabled`;

  const toolRow = (label: string, entries: CatalogEntry[]): SettingItem => ({
    id: `__tools:${label}`,
    label,
    currentValue: toolCount(entries),
    submenu: (_current, close) =>
      makeMultiSelectPanel(
        theme,
        label,
        [
          {
            label,
            items: entries.map((entry) => ({
              value: entry.id,
              label: entry.label,
              description: entry.description,
              selected: (definition.tools ?? []).includes(entry.id),
            })),
          },
        ],
        (values) => {
          if (values) {
            // Explicitly listed tools outside the current catalog stay listed.
            const allowed = new Set(entries.map((entry) => entry.id));
            const preserved = (definition.tools ?? []).filter((tool) => !allowed.has(tool));
            updateSelection(definition, "tools", new Set([...preserved, ...values]));
          }
          close(toolCount(entries));
        },
      ),
  });

  const catalogRow = (
    id: string,
    label: string,
    entries: CatalogEntry[],
    field: "models" | "extensions" | "skills",
    options: { maxSelections?: number } = {},
  ): SettingItem => ({
    id,
    label,
    currentValue: `${(definition[field] ?? []).length} selected`,
    submenu: (_current, close) =>
      makeMultiSelectPanel(
        theme,
        label,
        [
          {
            label,
            items: entries.map((entry) => ({
              value: entry.id,
              label: entry.label,
              description: entry.description,
              selected: (definition[field] ?? []).includes(entry.id),
            })),
          },
        ],
        (values) => {
          if (values) {
            // Models must come from the live catalog; resource paths may point
            // at entries the picker could not rediscover and are preserved.
            const keepUnknown =
              field === "models" ? [] : (definition[field] ?? []).filter((value) => !entries.some((entry) => entry.id === value));
            updateSelection(definition, field, new Set([...keepUnknown, ...values]));
          }
          close(`${(definition[field] ?? []).length} selected`);
        },
        options,
      ),
  });

  const items: SettingItem[] = [
    {
      id: "__enabled",
      label: "Enabled",
      currentValue: definition.enabled ? "on" : "off",
      values: ["on", "off"],
    },
    {
      id: "__description",
      label: "Description",
      currentValue: definition.description,
      submenu: (_current, close) =>
        makeInputPanel(
          theme,
          "Subagent description",
          (value) => {
            if (value !== undefined && value.trim()) definition.description = value.trim();
            close(definition.description);
          },
          { initial: definition.description },
        ),
    },
    {
      id: "__instructions",
      label: "Instructions",
      currentValue: `${definition.systemPrompt.split("\n").length} lines`,
      submenu: (_current, close) =>
        makeEditorPanel(tui, theme, "Subagent instructions", definition.systemPrompt, (value) => {
          if (value !== undefined) definition.systemPrompt = value.trim();
          close(`${definition.systemPrompt.split("\n").length} lines`);
        }),
    },
    toolRow("Built-in tools", catalog.builtin),
    toolRow("Plugin tools", catalog.plugin),
    catalogRow("__models", `Models (max ${MAX_AGENT_MODELS})`, models, "models", { maxSelections: MAX_AGENT_MODELS }),
    catalogRow("__extensions", "Extensions", extensionCatalog(pi), "extensions"),
    catalogRow("__skills", "Skills", skillCatalog(ctx.cwd), "skills"),
    {
      id: "__max_turns",
      label: "Max turns",
      currentValue: String(definition.maxTurns ?? 0),
      submenu: (_current, close) =>
        makeInputPanel(
          theme,
          "Max turns (0 = unlimited)",
          (value) => {
            const parsed = Number((value ?? "").trim());
            if (value !== undefined && Number.isInteger(parsed) && parsed >= 0) definition.maxTurns = parsed;
            close(String(definition.maxTurns ?? 0));
          },
          {
            initial: String(definition.maxTurns ?? 0),
            validate: (value) => {
              const parsed = Number(value.trim());
              return Number.isInteger(parsed) && parsed >= 0 ? undefined : "Max turns must be a non-negative integer.";
            },
          },
        ),
    },
    {
      id: "__restore",
      label: isBuiltinOverride ? restoreLabel : "Delete definition",
      currentValue: "",
      submenu: (_current, close) =>
        makeConfirmPanel(
          theme,
          isBuiltinOverride ? restoreLabel : "Delete subagent definition",
          isBuiltinOverride
            ? restoreLabel === "Restore built-in defaults"
              ? `Remove the global override for ${definition.name} and restore the built-in profile?`
              : `Remove the project override for ${definition.name} and restore its inherited definition?`
            : `Delete the ${definition.scope} subagent definition ${definition.name}?`,
          (confirmed) => close(confirmed ? "confirmed" : undefined),
        ),
    },
    { id: "__save", label: "✓ Save and close", currentValue: "", values: [""] },
    { id: "__discard", label: "✕ Discard changes", currentValue: "", values: [""] },
  ];

  const list = new SettingsList(
    items,
    Math.max(8, (tui.terminal?.rows ?? 24) - 6),
    settingsListTheme(theme),
    (id, value) => {
      if (id === "__enabled") {
        definition.enabled = value === "on";
        return;
      }
      if (id === "__save") {
        saveDefinition(ctx.cwd, definition);
        ctx.ui.notify(`Saved ${definition.scope} subagent ${definition.name}.`, "info");
        done("saved");
        return;
      }
      if (id === "__discard") {
        done(undefined);
        return;
      }
      if (id === "__restore" && value === "confirmed") {
        deleteDefinition(ctx.cwd, definition);
        ctx.ui.notify(
          isBuiltinOverride
            ? restoreLabel === "Restore built-in defaults"
              ? `Restored built-in subagent ${definition.name}.`
              : `Removed project override for ${definition.name}.`
            : `Deleted ${definition.scope} subagent ${definition.name}.`,
          "info",
        );
        done("deleted");
      }
    },
    () => done(undefined),
  );

  return new Panel(
    [
      new Text(styles.title(`Subagent ${definition.name} (${definition.scope})`)),
      new Text(styles.hint("Enter/click opens a field · Esc discards · Save persists")),
      list,
    ],
    list,
  );
}

async function editDefinition(
  pi: ExtensionAPI,
  ctx: ExtensionCommandContext,
  definition: AgentDefinition,
): Promise<void> {
  await openCustomPanel(ctx, (tui, theme, _keyboard, done) =>
    buildDefinitionForm(pi, ctx, tui as unknown as TUI, theme, definition, done),
  );
}

/**
 * `/subagents` entry point: pick a definition (or create one), then edit it in
 * the full-screen form. Built-in profiles require an explicit project/global
 * override so the shipped bundle stays read-only.
 */
export async function configureSubagents(pi: ExtensionAPI, ctx: ExtensionCommandContext): Promise<void> {
  const definitions = [...loadAgentDefinitions(ctx.cwd).values()].sort((left, right) =>
    left.name.localeCompare(right.name),
  );
  const choices = [
    { value: "__new_project", label: "New project subagent" },
    { value: "__new_global", label: "New global subagent" },
    {
      value: "__concurrency",
      label: `Background concurrency (${backgroundConcurrency()})…`,
      description: "Max simultaneous background subagent runs across the agent tree",
    },
    ...definitions.map((definition, index) => {
      const source = definition.scope === "builtin" ? "built-in" : definition.scope;
      const suffix =
        definition.scope === "builtin"
          ? " (default)"
          : definition.builtin
            ? " (built-in override)"
            : definition.enabled
              ? ""
              : " (disabled)";
      return {
        value: `def:${index}`,
        label: `${source}: ${definition.name}${suffix}`,
        description: definition.description,
      };
    }),
  ];

  const choice = await piSelect(ctx, "Subagent configuration", choices);
  if (choice === undefined) return;

  if (choice === "__concurrency") {
    await editBackgroundConcurrency(ctx);
    return;
  }

  if (choice === "__new_project" || choice === "__new_global") {
    const scope: AgentDefinitionScope = choice === "__new_project" ? "project" : "global";
    const rawName = await piInput(ctx, "Subagent name (letters, numbers, _ or -)", {
      validate: (value) => (definitionName(value) ? undefined : "Subagent name is invalid."),
    });
    const name = rawName && definitionName(rawName);
    if (!name) return;
    await editDefinition(pi, ctx, {
      name,
      scope,
      enabled: true,
      description: name,
      systemPrompt: profileFor("general-purpose").systemPrompt,
    });
    return;
  }

  const target = definitions[Number(choice.slice("def:".length))];
  if (!target) return;

  if (target.scope === "builtin") {
    const scopeChoice = await piSelect(ctx, `Create override for built-in ${target.name}`, [
      { value: "project", label: "Project override" },
      { value: "global", label: "Global override" },
    ]);
    if (!scopeChoice) return;
    await editDefinition(pi, ctx, {
      ...cloneDefinition(target),
      scope: scopeChoice === "project" ? "project" : "global",
      builtin: true,
    });
    return;
  }

  if (target.scope === "global") {
    const action = await piSelect(ctx, `Global subagent ${target.name}`, [
      { value: "edit", label: "Edit global definition" },
      { value: "disable", label: "Disable in this project" },
    ]);
    if (!action) return;
    if (action === "disable") {
      saveDefinition(ctx.cwd, {
        ...cloneDefinition(target),
        scope: "project",
        enabled: false,
      });
      ctx.ui.notify(`Disabled global subagent ${target.name} in this project.`, "info");
      return;
    }
  }

  await editDefinition(pi, ctx, cloneDefinition(target));
}
