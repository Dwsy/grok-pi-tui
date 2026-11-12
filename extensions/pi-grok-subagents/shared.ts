/** Shared constants, capability tables, and scalar helpers for pi-grok-subagents. */

import { randomBytes } from "node:crypto";

export const BRIDGE_TYPE = "pi-grok-subagent/v1";
export const SUBAGENT_REPLAY_COMMAND = "__pi_grok_subagent_replay";
export const MAX_BACKGROUND_CONCURRENCY = 25;
export const MAX_WAIT_MS = 600_000; // 10 minutes cap for blocking waits
export const MAX_AGENT_MODELS = 3;
export const SHORT_SUBAGENT_ID_LENGTH = 8;

export const BUILTIN_TOOL_NAMES = ["read", "bash", "edit", "write", "grep", "find", "ls"] as const;
export const BUILTIN_TOOL_SET = new Set<string>(BUILTIN_TOOL_NAMES);

/**
 * Two capability modes: `read-only` omits edit/write and injects
 * READ_ONLY_CONTRACT; `all` gets every built-in tool. Both include Bash;
 * beyond that reduced loadout, behaviour is constrained by the injected prompt.
 */
export const CAPABILITY_TOOLS = {
  "read-only": ["read", "bash", "grep", "find", "ls"],
  all: [...BUILTIN_TOOL_NAMES],
} as const;

export type CapabilityMode = keyof typeof CAPABILITY_TOOLS;

export const READ_ONLY_SUBAGENT_PROMPT = [
  "READ-ONLY CONTRACT — REQUIRED: This subagent is strictly observational. Bash is available for inspection only; its availability is not permission to modify anything.",
  "Do not create, edit, overwrite, append to, truncate, delete, rename, move, copy, or otherwise write any file or directory, including temporary files, caches, generated artifacts, or files outside the workspace.",
  "Do not run commands or scripts that mutate filesystem, Git, database, service, process-control, package, environment, or remote-system state. This includes file redirection, tee, write-producing heredocs, sed -i, perl -i, patch/apply tools, file copy/move/delete commands, permission changes, starting/stopping/killing services or processes, package installation or updates, database writes, remote write requests, and Git mutations such as add, commit, fetch, pull, push, reset, checkout, switch, clean, stash, cherry-pick, merge, rebase, apply, restore, tag, update-ref, or worktree add/remove.",
  "Python, Node.js, and other scripting runtimes may be used only for in-memory analysis and read-only inspection; never open files for writing or create temporary outputs. Do not run build, test, format, or lint commands when they generate or rewrite artifacts.",
  "Use only commands whose effects are observational and leave local and external state unchanged. If the task requires a modification or stateful verification, stop and report that a write-enabled agent must handle it; do not attempt the change through Bash.",
].join(" ");

export function resolveSubagentBusinessTools(
  configuredTools: readonly string[],
  capabilityMode: CapabilityMode,
  explicitToolList = false,
): { tools: string[]; readOnly: boolean } {
  const readOnly = capabilityMode === "read-only"
    || (explicitToolList && !configuredTools.includes("edit") && !configuredTools.includes("write"));
  return {
    // Definition-supplied tool lists keep Bash too; the read-only contract is
    // carried by the injected prompt, not by removing Bash.
    tools: [...new Set([...configuredTools, "bash"])],
    readOnly,
  };
}

export function composeSubagentSystemPrompt(basePrompt: string, readOnly: boolean, suffix?: string): string {
  return [
    basePrompt.trim(),
    suffix?.trim() ?? "",
    readOnly ? READ_ONLY_SUBAGENT_PROMPT : "",
  ].filter(Boolean).join("\n\n");
}

export type CatalogEntry = {
  id: string;
  label: string;
  description?: string;
};

export type InjectedExtensionCatalogEntry = {
  path: string;
  label: string;
  description?: string;
};

/**
 * Session-scoped subagent ID: exactly SHORT_SUBAGENT_ID_LENGTH hex chars, so
 * existing "8+ character prefix" resolution keeps working unchanged while full
 * IDs stay short in persisted records and UI surfaces (36-char UUID → 8 chars).
 */
export function newSubagentId(): string {
  return randomBytes(SHORT_SUBAGENT_ID_LENGTH / 2).toString("hex");
}

export function requireText(value: unknown, name: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${name} is required`);
  }
  return value.trim();
}

export function requireCapability(value: string | undefined): CapabilityMode {
  const capability = value ?? "all";
  if (capability === "read-only" || capability === "all") return capability;
  // Existing presets still pass execute/read-write; collapse them into all.
  if (capability === "execute" || capability === "read-write") return "all";
  throw new Error(`unsupported capability_mode: ${capability}`);
}

export function textFromContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((block): block is { type: string; text?: unknown } => typeof block === "object" && block !== null)
    .filter((block) => block.type === "text" && typeof block.text === "string")
    .map((block) => block.text as string)
    .join("");
}

export function extractUsage(message: unknown): number {
  if (typeof message !== "object" || message === null) return 0;
  const usage = (message as { usage?: unknown }).usage;
  if (typeof usage !== "object" || usage === null) return 0;
  const input = (usage as { input?: unknown }).input;
  const output = (usage as { output?: unknown }).output;
  return (typeof input === "number" ? input : 0) + (typeof output === "number" ? output : 0);
}
