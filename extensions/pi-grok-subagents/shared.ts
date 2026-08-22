/** Shared constants, capability tables, and scalar helpers for pi-grok-subagents. */

export const BRIDGE_TYPE = "pi-grok-subagent/v1";
export const STATE_ENTRY_TYPE = "pi-grok-subagent-state/v1";
export const MAX_BACKGROUND_CONCURRENCY = 4;
export const MAX_WAIT_MS = 600_000; // 10 minutes cap for blocking waits
export const MAX_AGENT_MODELS = 3;
export const SHORT_SUBAGENT_ID_LENGTH = 8;
export const MULTI_SELECT_TITLE_PREFIX = "__pi_grok_multi_select_v1__:";
export const RESOURCE_PICKER_TITLE_PREFIX = "__pi_grok_resource_picker_v1__:";

export const BUILTIN_TOOL_NAMES = ["read", "bash", "edit", "write", "grep", "find", "ls"] as const;
export const BUILTIN_TOOL_SET = new Set<string>(BUILTIN_TOOL_NAMES);

export const CAPABILITY_TOOLS = {
  "read-only": ["read", "grep", "find", "ls"],
  "read-write": ["read", "grep", "find", "ls", "edit", "write"],
  execute: ["read", "bash", "grep", "find", "ls"],
  all: [...BUILTIN_TOOL_NAMES],
} as const;

export type CapabilityMode = keyof typeof CAPABILITY_TOOLS;

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

export type ResourcePickerExtra = {
  path: string;
  label: string;
  type: "extensions" | "skills";
};

export function requireText(value: unknown, name: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${name} is required`);
  }
  return value.trim();
}

export function requireCapability(value: string | undefined): CapabilityMode {
  const capability = value ?? "all";
  if (!(capability in CAPABILITY_TOOLS)) {
    throw new Error(`unsupported capability_mode: ${capability}`);
  }
  return capability as CapabilityMode;
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
